//! Page-number and cursor pagination on top of Toasty list queries.
//!
//! Both flavors return contract-friendly value objects that serialize with
//! camelCase field names, matching the Resource conventions in
//! `docs/CONTRACTS.md`:
//!
//! - [`Paginated<T>`] — page numbers plus a `meta` block with
//!   `currentPage` / `perPage` / `total` / `lastPage`.
//! - [`CursorPaginated<T>`] — an opaque `nextCursor` for stable infinite
//!   scrolling over a monotonic sort key.
//!
//! Use [`QueryPagination`] on any Toasty list query (`Model::all()`, generated
//! finder structs, or `stmt::Query<List<M>>`), then [`Paginated::map`] /
//! [`CursorPaginated::map`] to convert database models into Resources before
//! anything reaches the browser.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use toasty::Executor;
use toasty::schema::Model;
use toasty::stmt::{IntoStatement, List, OrderBy, Paginate, Query, Value};

/// Default upper bound applied to `per_page` when using
/// [`QueryPagination::page_paginate`] or [`QueryPagination::cursor_paginate`].
pub const DEFAULT_MAX_PER_PAGE: u64 = 100;

/// The largest `per_page` any normalization can produce, keeping the value
/// convertible to the `i64` SQL limit Toasty binds.
const MAX_SQL_LIMIT: u64 = i64::MAX.unsigned_abs();

/// A page-numbered result set ready to become a Resource contract payload.
///
/// Serializes as `{ "data": [...], "meta": { ... } }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Paginated<T> {
    /// Items on the current page.
    pub data: Vec<T>,
    /// Page-number metadata (`currentPage`, `perPage`, `total`, `lastPage`).
    pub meta: PageMeta,
}

impl<T> Paginated<T> {
    /// Convert every item — typically database model to Resource — while
    /// keeping the pagination metadata unchanged.
    #[must_use]
    pub fn map<U>(self, transform: impl FnMut(T) -> U) -> Paginated<U> {
        Paginated {
            data: self.data.into_iter().map(transform).collect(),
            meta: self.meta,
        }
    }
}

/// Metadata for page-number pagination.
///
/// Field names serialize in camelCase (`currentPage`, `perPage`, `total`,
/// `lastPage`) to match the Resource contract conventions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMeta {
    /// The 1-based page that was fetched (after normalization).
    pub current_page: u64,
    /// Items per page after clamping to `1..=max_per_page`.
    pub per_page: u64,
    /// Total number of rows matching the query.
    pub total: u64,
    /// The last page number; at least `1` even for an empty result.
    pub last_page: u64,
}

/// A cursor-paginated result set ready to become a Resource contract payload.
///
/// Serializes as `{ "data": [...], "meta": { "perPage": ..., "nextCursor": ... } }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CursorPaginated<T> {
    /// Items on the current page.
    pub data: Vec<T>,
    /// Cursor metadata (`perPage`, `nextCursor`).
    pub meta: CursorPageMeta,
}

impl<T> CursorPaginated<T> {
    /// Convert every item — typically database model to Resource — while
    /// keeping the cursor metadata unchanged.
    #[must_use]
    pub fn map<U>(self, transform: impl FnMut(T) -> U) -> CursorPaginated<U> {
        CursorPaginated {
            data: self.data.into_iter().map(transform).collect(),
            meta: self.meta,
        }
    }
}

/// Metadata for cursor pagination.
///
/// `next_cursor` serializes as `nextCursor`; it is an opaque base64 token that
/// clients must echo back unchanged. `None` means a page returned fewer than
/// `per_page` rows, i.e. the result set is exhausted. A result set whose size
/// is an exact multiple of `per_page` yields one final `Some` cursor whose
/// next page is empty.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorPageMeta {
    /// Items per page after clamping to `1..=max_per_page`.
    pub per_page: u64,
    /// Opaque token for fetching the next page, or `None` when exhausted.
    pub next_cursor: Option<String>,
}

/// Pagination failure with stable Phoenix categories.
#[derive(Debug, Error)]
pub enum PaginationError {
    /// The statement was not a select query (e.g. an insert or delete).
    #[error("pagination requires a select query")]
    NotASelect,
    /// The client-supplied cursor was not produced by
    /// [`QueryPagination::cursor_paginate`] or was corrupted.
    #[error("invalid pagination cursor")]
    InvalidCursor,
    /// The sort key type cannot be encoded into an opaque cursor yet.
    #[error("unsupported cursor key type `{0}`; order by an integer or string column")]
    UnsupportedCursorKey(&'static str),
    /// The underlying database operation failed.
    #[error("database operation failed: {0}")]
    Toasty(#[from] toasty::Error),
}

/// Pagination over any Toasty list query.
///
/// Implemented for every builder that converts into a list select — including
/// `Model::all()` / generated finder structs and raw
/// [`Query<List<M>>`](toasty::stmt::Query).
///
/// The query passed in must not already carry `limit` / `offset` / `order_by`
/// clauses: ordering is supplied through the `order` argument so the row
/// count can run without an `ORDER BY` (which `PostgreSQL` and `MySQL` reject in
/// aggregate queries).
#[allow(async_fn_in_trait)]
pub trait QueryPagination<M: Model>: Sized {
    /// Page-number pagination with the default `per_page` cap of
    /// [`DEFAULT_MAX_PER_PAGE`].
    ///
    /// `page` is 1-based; `0` is normalized to `1`. `per_page` is clamped to
    /// `1..=DEFAULT_MAX_PER_PAGE`. A `page` past the last page returns empty
    /// `data` with accurate `meta`. The row count and the page query run
    /// sequentially on the same executor.
    ///
    /// # Errors
    ///
    /// Returns [`PaginationError::NotASelect`] when the statement is not a
    /// select, and [`PaginationError::Toasty`] when the database fails.
    ///
    /// # Panics
    ///
    /// Panics if the query already has a `limit` or `order_by` clause.
    async fn page_paginate(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        page: u64,
        per_page: u64,
    ) -> Result<Paginated<M>, PaginationError> {
        self.page_paginate_with_max(executor, order, page, per_page, DEFAULT_MAX_PER_PAGE)
            .await
    }

    /// Page-number pagination with a custom `per_page` upper bound.
    ///
    /// Semantics match [`QueryPagination::page_paginate`]; `per_page` is clamped to
    /// `1..=max_per_page` (a `max_per_page` of `0` is treated as `1`).
    ///
    /// # Errors
    ///
    /// Returns [`PaginationError::NotASelect`] when the statement is not a
    /// select, and [`PaginationError::Toasty`] when the database fails.
    ///
    /// # Panics
    ///
    /// Panics if the query already has a `limit` or `order_by` clause.
    async fn page_paginate_with_max(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        page: u64,
        per_page: u64,
        max_per_page: u64,
    ) -> Result<Paginated<M>, PaginationError>;

    /// Cursor pagination with the default `per_page` cap of
    /// [`DEFAULT_MAX_PER_PAGE`].
    ///
    /// Pass `None` as `cursor` for the first page, then echo back
    /// `meta.next_cursor` until it is `None`. The cursor is an opaque base64
    /// token derived from the `order` key values of the last row; the first
    /// version supports ordering by monotonic integer or string columns
    /// (primary keys, ISO-8601 time strings).
    ///
    /// # Errors
    ///
    /// Returns [`PaginationError::InvalidCursor`] for a token this API did not
    /// produce, [`PaginationError::UnsupportedCursorKey`] when the sort key
    /// type cannot be encoded, [`PaginationError::NotASelect`] when the
    /// statement is not a select, and [`PaginationError::Toasty`] when the
    /// database fails.
    ///
    /// # Panics
    ///
    /// Panics if the query already has a `limit` clause.
    async fn cursor_paginate(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        cursor: Option<String>,
        per_page: u64,
    ) -> Result<CursorPaginated<M>, PaginationError> {
        self.cursor_paginate_with_max(executor, order, cursor, per_page, DEFAULT_MAX_PER_PAGE)
            .await
    }

    /// Cursor pagination with a custom `per_page` upper bound.
    ///
    /// Semantics match [`QueryPagination::cursor_paginate`]; `per_page` is
    /// clamped to `1..=max_per_page` (a `max_per_page` of `0` is treated as
    /// `1`).
    ///
    /// # Errors
    ///
    /// Returns [`PaginationError::InvalidCursor`] for a token this API did not
    /// produce, [`PaginationError::UnsupportedCursorKey`] when the sort key
    /// type cannot be encoded, [`PaginationError::NotASelect`] when the
    /// statement is not a select, and [`PaginationError::Toasty`] when the
    /// database fails.
    ///
    /// # Panics
    ///
    /// Panics if the query already has a `limit` clause.
    async fn cursor_paginate_with_max(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        cursor: Option<String>,
        per_page: u64,
        max_per_page: u64,
    ) -> Result<CursorPaginated<M>, PaginationError>;
}

impl<M, Q> QueryPagination<M> for Q
where
    M: Model,
    Q: IntoStatement<Returning = List<M>>,
{
    async fn page_paginate_with_max(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        page: u64,
        per_page: u64,
        max_per_page: u64,
    ) -> Result<Paginated<M>, PaginationError> {
        let query = into_list_query(self)?;
        let page = normalize_page(page);
        let per_page = normalize_per_page(per_page, max_per_page);

        let total = query.clone().count().exec(executor).await?;
        let last_page = total.div_ceil(per_page).max(1);

        let offset = (page - 1).checked_mul(per_page);
        let data = match offset {
            Some(offset) if offset < total => {
                query
                    .order_by(order)
                    .limit(to_usize(per_page))
                    .offset(to_usize(offset))
                    .exec(executor)
                    .await?
            }
            _ => Vec::new(),
        };

        Ok(Paginated {
            data,
            meta: PageMeta {
                current_page: page,
                per_page,
                total,
                last_page,
            },
        })
    }

    async fn cursor_paginate_with_max(
        self,
        executor: &mut dyn Executor,
        order: impl Into<OrderBy>,
        cursor: Option<String>,
        per_page: u64,
        max_per_page: u64,
    ) -> Result<CursorPaginated<M>, PaginationError> {
        let query = into_list_query(self)?;
        let per_page = normalize_per_page(per_page, max_per_page);

        let mut paginate = Paginate::new(query.order_by(order), to_usize(per_page));
        if let Some(encoded) = cursor {
            paginate = paginate.after(decode_cursor(&encoded)?);
        }

        let page = paginate.exec(executor).await?;
        let next_cursor = page.next_cursor.as_ref().map(encode_cursor).transpose()?;

        Ok(CursorPaginated {
            data: page.items,
            meta: CursorPageMeta {
                per_page,
                next_cursor,
            },
        })
    }
}

fn into_list_query<M, Q>(builder: Q) -> Result<Query<List<M>>, PaginationError>
where
    M: Model,
    Q: IntoStatement<Returning = List<M>>,
{
    builder
        .into_statement()
        .into_query()
        .ok_or(PaginationError::NotASelect)
}

const fn normalize_page(page: u64) -> u64 {
    if page == 0 { 1 } else { page }
}

fn normalize_per_page(per_page: u64, max_per_page: u64) -> u64 {
    per_page.clamp(1, max_per_page.clamp(1, MAX_SQL_LIMIT))
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// One sort-key value inside an opaque cursor.
///
/// The externally tagged serde representation keeps the exact Toasty `Value`
/// variant so the decoded cursor binds with the same SQL type it was read
/// with.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CursorKey {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    Str(String),
    Null,
}

fn key_from_value(value: &Value) -> Result<CursorKey, PaginationError> {
    Ok(match value {
        Value::Bool(inner) => CursorKey::Bool(*inner),
        Value::I8(inner) => CursorKey::I8(*inner),
        Value::I16(inner) => CursorKey::I16(*inner),
        Value::I32(inner) => CursorKey::I32(*inner),
        Value::I64(inner) => CursorKey::I64(*inner),
        Value::U8(inner) => CursorKey::U8(*inner),
        Value::U16(inner) => CursorKey::U16(*inner),
        Value::U32(inner) => CursorKey::U32(*inner),
        Value::U64(inner) => CursorKey::U64(*inner),
        Value::String(inner) => CursorKey::Str(inner.clone()),
        Value::Null => CursorKey::Null,
        other => return Err(PaginationError::UnsupportedCursorKey(value_kind(other))),
    })
}

fn key_into_value(key: CursorKey) -> Value {
    match key {
        CursorKey::Bool(inner) => Value::Bool(inner),
        CursorKey::I8(inner) => Value::I8(inner),
        CursorKey::I16(inner) => Value::I16(inner),
        CursorKey::I32(inner) => Value::I32(inner),
        CursorKey::I64(inner) => Value::I64(inner),
        CursorKey::U8(inner) => Value::U8(inner),
        CursorKey::U16(inner) => Value::U16(inner),
        CursorKey::U32(inner) => Value::U32(inner),
        CursorKey::U64(inner) => Value::U64(inner),
        CursorKey::Str(inner) => Value::String(inner),
        CursorKey::Null => Value::Null,
    }
}

// Toasty gates some `Value` variants (decimal / jiff time types) behind Cargo
// features; the wildcard arm is unreachable unless one of them is enabled.
#[allow(unreachable_patterns)]
const fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "bool",
        Value::I8(_) => "i8",
        Value::I16(_) => "i16",
        Value::I32(_) => "i32",
        Value::I64(_) => "i64",
        Value::U8(_) => "u8",
        Value::U16(_) => "u16",
        Value::U32(_) => "u32",
        Value::U64(_) => "u64",
        Value::F32(_) => "f32",
        Value::F64(_) => "f64",
        Value::String(_) => "string",
        Value::Bytes(_) => "bytes",
        Value::Uuid(_) => "uuid",
        Value::Record(_) => "record",
        Value::SparseRecord(_) => "sparse record",
        Value::List(_) => "list",
        Value::Null => "null",
        _ => "unsupported",
    }
}

fn encode_cursor(value: &Value) -> Result<String, PaginationError> {
    let keys: Vec<CursorKey> = match value {
        Value::Record(record) => record
            .fields
            .iter()
            .map(key_from_value)
            .collect::<Result<_, _>>()?,
        scalar => vec![key_from_value(scalar)?],
    };
    let json = serde_json::to_vec(&keys).map_err(|_| PaginationError::InvalidCursor)?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_cursor(encoded: &str) -> Result<Value, PaginationError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| PaginationError::InvalidCursor)?;
    let keys: Vec<CursorKey> =
        serde_json::from_slice(&bytes).map_err(|_| PaginationError::InvalidCursor)?;
    if keys.is_empty() {
        return Err(PaginationError::InvalidCursor);
    }
    Ok(Value::record_from_vec(
        keys.into_iter().map(key_into_value).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_zero_normalizes_to_one() {
        assert_eq!(normalize_page(0), 1);
        assert_eq!(normalize_page(1), 1);
        assert_eq!(normalize_page(7), 7);
    }

    #[test]
    fn per_page_clamps_into_configured_bounds() {
        assert_eq!(normalize_per_page(0, DEFAULT_MAX_PER_PAGE), 1);
        assert_eq!(normalize_per_page(15, DEFAULT_MAX_PER_PAGE), 15);
        assert_eq!(
            normalize_per_page(1_000, DEFAULT_MAX_PER_PAGE),
            DEFAULT_MAX_PER_PAGE
        );
        assert_eq!(normalize_per_page(10, 5), 5);
        assert_eq!(normalize_per_page(10, 0), 1);
        assert_eq!(normalize_per_page(u64::MAX, u64::MAX), MAX_SQL_LIMIT);
    }

    #[test]
    fn cursor_round_trips_supported_key_types() {
        let original = Value::record_from_vec(vec![
            Value::U64(42),
            Value::I64(-7),
            Value::String("2026-07-26T00:00:00Z".to_owned()),
            Value::Null,
        ]);
        let encoded = encode_cursor(&original).unwrap();
        assert_eq!(decode_cursor(&encoded).unwrap(), original);
    }

    #[test]
    fn cursor_rejects_unsupported_key_types() {
        let error = encode_cursor(&Value::F64(1.5)).unwrap_err();
        assert!(matches!(
            error,
            PaginationError::UnsupportedCursorKey("f64")
        ));
    }

    #[test]
    fn cursor_rejects_garbage_tokens() {
        for garbage in ["not base64 !!", "bm90IGpzb24", ""] {
            assert!(matches!(
                decode_cursor(garbage).unwrap_err(),
                PaginationError::InvalidCursor
            ));
        }
    }
}
