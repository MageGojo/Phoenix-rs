use phoenix_database::{
    DEFAULT_MAX_PER_PAGE, Model, PageMeta, Paginated, PaginationError, QueryPagination,
    TestDatabase, models,
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Model)]
struct Item {
    #[key]
    #[auto]
    id: u64,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemResource {
    id: u64,
    name: String,
}

impl From<Item> for ItemResource {
    fn from(item: Item) -> Self {
        Self {
            id: item.id,
            name: item.name,
        }
    }
}

async fn seeded(count: usize) -> TestDatabase {
    let mut database = TestDatabase::new(models!(Item)).await.unwrap();
    for index in 0..count {
        Item::create()
            .name(format!("item-{index:02}"))
            .exec(database.toasty_mut())
            .await
            .unwrap();
    }
    database
}

fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

fn order() -> impl Into<phoenix_database::stmt::OrderBy> {
    Item::fields().id().asc()
}

#[tokio::test]
async fn empty_table_yields_an_empty_first_page() {
    let mut database = seeded(0).await;

    let page = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 10)
        .await
        .unwrap();

    assert!(page.data.is_empty());
    assert_eq!(
        page.meta,
        PageMeta {
            current_page: 1,
            per_page: 10,
            total: 0,
            last_page: 1,
        }
    );
}

#[tokio::test]
async fn splits_an_exact_multiple_into_full_pages() {
    let mut database = seeded(6).await;

    let first = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 3)
        .await
        .unwrap();
    assert_eq!(names(&first.data), ["item-00", "item-01", "item-02"]);
    assert_eq!(
        first.meta,
        PageMeta {
            current_page: 1,
            per_page: 3,
            total: 6,
            last_page: 2,
        }
    );

    let second = Item::all()
        .page_paginate(database.toasty_mut(), order(), 2, 3)
        .await
        .unwrap();
    assert_eq!(names(&second.data), ["item-03", "item-04", "item-05"]);
    assert_eq!(second.meta.current_page, 2);
    assert_eq!(second.meta.last_page, 2);
}

#[tokio::test]
async fn final_page_holds_the_remainder() {
    let mut database = seeded(7).await;

    let last = Item::all()
        .page_paginate(database.toasty_mut(), order(), 3, 3)
        .await
        .unwrap();
    assert_eq!(names(&last.data), ["item-06"]);
    assert_eq!(
        last.meta,
        PageMeta {
            current_page: 3,
            per_page: 3,
            total: 7,
            last_page: 3,
        }
    );
}

#[tokio::test]
async fn a_page_past_the_end_is_empty_with_accurate_meta() {
    let mut database = seeded(4).await;

    let page = Item::all()
        .page_paginate(database.toasty_mut(), order(), 99, 3)
        .await
        .unwrap();

    assert!(page.data.is_empty());
    assert_eq!(
        page.meta,
        PageMeta {
            current_page: 99,
            per_page: 3,
            total: 4,
            last_page: 2,
        }
    );
}

#[tokio::test]
async fn page_and_per_page_inputs_are_normalized() {
    let mut database = seeded(3).await;

    // page 0 behaves as page 1.
    let page = Item::all()
        .page_paginate(database.toasty_mut(), order(), 0, 2)
        .await
        .unwrap();
    assert_eq!(page.meta.current_page, 1);
    assert_eq!(names(&page.data), ["item-00", "item-01"]);

    // per_page 0 becomes 1.
    let page = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 0)
        .await
        .unwrap();
    assert_eq!(page.meta.per_page, 1);
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.meta.last_page, 3);

    // per_page above the default cap clamps to DEFAULT_MAX_PER_PAGE.
    let page = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 10_000)
        .await
        .unwrap();
    assert_eq!(page.meta.per_page, DEFAULT_MAX_PER_PAGE);

    // A custom cap wins over the requested per_page.
    let page = Item::all()
        .page_paginate_with_max(database.toasty_mut(), order(), 1, 10, 2)
        .await
        .unwrap();
    assert_eq!(page.meta.per_page, 2);
    assert_eq!(page.data.len(), 2);
    assert_eq!(page.meta.last_page, 2);
}

#[tokio::test]
async fn cursor_pagination_walks_the_full_result_set() {
    let mut database = seeded(7).await;

    let mut cursor = None;
    let mut sizes = Vec::new();
    let mut seen = Vec::new();

    loop {
        let page = Item::all()
            .cursor_paginate(database.toasty_mut(), order(), cursor.take(), 3)
            .await
            .unwrap();
        assert_eq!(page.meta.per_page, 3);
        sizes.push(page.data.len());
        seen.extend(page.data.into_iter().map(|item| item.name));
        match page.meta.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(sizes, [3, 3, 1]);
    let expected: Vec<String> = (0..7).map(|index| format!("item-{index:02}")).collect();
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn cursor_pagination_over_an_exact_multiple_ends_with_an_empty_page() {
    let mut database = seeded(6).await;

    let first = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), None, 3)
        .await
        .unwrap();
    let second = Item::all()
        .cursor_paginate(
            database.toasty_mut(),
            order(),
            first.meta.next_cursor.clone(),
            3,
        )
        .await
        .unwrap();
    assert_eq!(names(&second.data), ["item-03", "item-04", "item-05"]);

    // The final full page still reports a cursor; the page after it is empty.
    let last = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), second.meta.next_cursor, 3)
        .await
        .unwrap();
    assert!(last.data.is_empty());
    assert!(last.meta.next_cursor.is_none());
}

#[tokio::test]
async fn cursor_pagination_of_an_empty_table_has_no_next_cursor() {
    let mut database = seeded(0).await;

    let page = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), None, 5)
        .await
        .unwrap();

    assert!(page.data.is_empty());
    assert!(page.meta.next_cursor.is_none());
}

#[tokio::test]
async fn a_tampered_cursor_is_rejected() {
    let mut database = seeded(2).await;

    let error = Item::all()
        .cursor_paginate(
            database.toasty_mut(),
            order(),
            Some("definitely !! not a cursor".to_owned()),
            5,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PaginationError::InvalidCursor));
}

#[tokio::test]
async fn map_converts_models_and_serializes_with_camel_case_meta() {
    let mut database = seeded(3).await;

    let resources: Paginated<ItemResource> = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 2)
        .await
        .unwrap()
        .map(ItemResource::from)
        .map(|resource| ItemResource {
            name: resource.name.to_uppercase(),
            ..resource
        });

    assert_eq!(
        serde_json::to_value(&resources).unwrap(),
        json!({
            "data": [
                { "id": resources.data[0].id, "name": "ITEM-00" },
                { "id": resources.data[1].id, "name": "ITEM-01" },
            ],
            "meta": {
                "currentPage": 1,
                "perPage": 2,
                "total": 3,
                "lastPage": 2,
            },
        })
    );

    let cursor_page = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), None, 2)
        .await
        .unwrap()
        .map(ItemResource::from);
    let serialized = serde_json::to_value(&cursor_page).unwrap();
    assert_eq!(serialized["meta"]["perPage"], json!(2));
    assert!(serialized["meta"]["nextCursor"].is_string());
    assert_eq!(serialized["data"][0]["name"], json!("item-00"));
}

/// The wire shape of these two wrappers is duplicated in the TypeScript
/// contract generator (`FRAMEWORK_GENERICS` in
/// `packages/phoenix-vite/src/contracts.ts`), which emits `PhoenixPaginated<T>`
/// / `PhoenixCursorPaginated<T>` without reading these structs. Pin the exact
/// key sets here so a Rust-side rename cannot silently drift away from the
/// types the browser is compiled against.
#[tokio::test]
async fn wrapper_wire_keys_match_the_typescript_generator() {
    let mut database = seeded(3).await;

    let page: Paginated<ItemResource> = Item::all()
        .page_paginate(database.toasty_mut(), order(), 1, 2)
        .await
        .unwrap()
        .map(ItemResource::from);
    let value = serde_json::to_value(&page).unwrap();
    assert_eq!(keys(&value), vec!["data", "meta"]);
    assert_eq!(
        keys(&value["meta"]),
        vec!["currentPage", "lastPage", "perPage", "total"]
    );
    for key in ["currentPage", "lastPage", "perPage", "total"] {
        assert!(
            value["meta"][key].is_u64(),
            "{key} must serialize as a JSON number"
        );
    }

    let cursor = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), None, 2)
        .await
        .unwrap()
        .map(ItemResource::from);
    let value = serde_json::to_value(&cursor).unwrap();
    assert_eq!(keys(&value), vec!["data", "meta"]);
    assert_eq!(keys(&value["meta"]), vec!["nextCursor", "perPage"]);
    assert!(value["meta"]["perPage"].is_u64());

    // `nextCursor` is `string | null` in TypeScript: null once exhausted.
    let exhausted = Item::all()
        .cursor_paginate(database.toasty_mut(), order(), None, 10)
        .await
        .unwrap()
        .map(ItemResource::from);
    assert_eq!(
        serde_json::to_value(&exhausted).unwrap()["meta"]["nextCursor"],
        json!(null)
    );
}

/// Sorted object keys, so assertions are order-independent.
fn keys(value: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}
