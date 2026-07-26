//! Marker and convention macros for Phoenix code generation.

use proc_macro::TokenStream;
use syn::{ItemStruct, parse_macro_input};

mod model;

/// Mark a Rust DTO as a Phoenix input, resource, page, or shared contract.
///
/// The attribute is intentionally representation-neutral. The Vite contract
/// exporter reads the Rust declaration and applies Serde's wire rules.
#[proc_macro_attribute]
pub fn contract(_metadata: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Declare a database model with Phoenix's conventions filled in.
///
/// Expands to a plain Toasty model, so anything Toasty can express is still
/// reachable — the attribute only supplies the parts that are the same in
/// almost every model, and steps aside the moment you write them yourself.
///
/// # Conventions
///
/// | Written | Filled in |
/// |---|---|
/// | *(nothing)* | `#[table = "..."]` — `snake_case`, pluralized from the type name |
/// | *(nothing)* | `#[key] #[auto] pub id: i64` |
/// | *(nothing)* | `#[derive(Debug, Model)]` |
/// | `#[belongs_to] pub user: Deferred<User>` | `key = user_id`, `references = id`, and the `user_id: i64` field |
///
/// # Example
///
/// ```ignore
/// #[phoenix::model]
/// pub struct Post {
///     pub title: String,
///     pub body: String,
///     #[belongs_to]                       // 只需说明「属于 User」
///     pub user: Deferred<User>,
/// }
/// ```
///
/// is the same model as:
///
/// ```ignore
/// #[derive(Debug, Model)]
/// #[table = "posts"]
/// pub struct Post {
///     #[key]
///     #[auto]
///     pub id: i64,
///     pub title: String,
///     pub body: String,
///     pub user_id: i64,
///     #[belongs_to(key = user_id, references = id)]
///     pub user: Deferred<User>,
/// }
/// ```
///
/// # Overriding
///
/// Every convention yields to an explicit declaration — write `#[table = …]`,
/// your own `#[key]` field, `#[belongs_to(key = author_id)]`, or the foreign-key
/// field itself, and this attribute leaves that part alone. A nullable relation
/// (`Deferred<Option<User>>`) gets a nullable foreign key.
///
/// `#[has_many]` and `#[has_one]` are passed through untouched: Toasty already
/// infers their pairing from the target's `#[belongs_to]`, so there is no
/// convention left to add.
///
/// The generated primary key and foreign keys are `i64`. A model keyed by
/// anything else declares its own foreign-key fields — this attribute cannot
/// see the target model's key type, and guessing it would surface as a
/// confusing error deep inside the Toasty derive.
#[proc_macro_attribute]
pub fn model(metadata: TokenStream, item: TokenStream) -> TokenStream {
    if !metadata.is_empty() {
        return syn::Error::new(
            model::call_site(),
            "#[phoenix::model] takes no arguments; use #[table = \"...\"] to name the table",
        )
        .to_compile_error()
        .into();
    }
    let item = parse_macro_input!(item as ItemStruct);
    model::expand(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
