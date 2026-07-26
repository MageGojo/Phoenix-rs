//! End-to-end test of the model conventions and factories against `SQLite`.
//!
//! This is the only place both halves meet as an application sees them:
//! `#[phoenix::model]` writing the relation, and `phoenix::factory!` filling it
//! with rows. Everything below is written exactly as it would be in a generated
//! project — if the conventions were wrong, this file would not compile.
//!
//! Run with: `cargo test -p phoenixrs --features sqlite,factory`

#![cfg(all(feature = "factory", feature = "sqlite"))]

use phoenix::database::factory::{Faker, Locale, Seeder};
use phoenix::database::{Deferred, TestDatabase, models};

/// A user. No `id`, no `#[table]`, no derives — all conventions.
#[phoenix::model]
pub struct User {
    pub name: String,
    pub email: String,
}

/// A post that belongs to a user. The relation is one line: **which model**,
/// nothing else. `user_id` and the key mapping are filled in.
#[phoenix::model]
pub struct Post {
    pub title: String,
    pub body: String,
    #[belongs_to]
    pub user: Deferred<User>,
}

/// A comment showing the manual escape hatch: a renamed foreign key, a
/// nullable relation, and a table name that is not the pluralized type.
#[phoenix::model]
#[table = "post_comments"]
pub struct Comment {
    pub body: String,
    #[belongs_to(key = post_id)]
    pub post: Deferred<Post>,
    #[belongs_to]
    pub author: Deferred<Option<User>>,
}

phoenix::factory! {
    User, |f| User::create()
        .name(f.name())
        .email(f.unique_email()),
}

phoenix::factory! {
    Post, |f, user_id: i64| Post::create()
        .title(f.sentence(6))
        .body(f.paragraph(2))
        .user_id(user_id),
}

async fn database() -> phoenix::database::Database {
    TestDatabase::new(models!(User, Post, Comment))
        .await
        .expect("test database")
        .into_database()
}

#[tokio::test]
async fn the_conventions_produce_a_working_relation() {
    let mut database = database().await;

    let user = User::create()
        .name("Ada")
        .email("ada@example.com")
        .exec(database.toasty_mut())
        .await
        .expect("create user");
    // `user_id` exists because the relation declared it.
    let post = Post::create()
        .title("Hello")
        .body("World")
        .user_id(user.id)
        .exec(database.toasty_mut())
        .await
        .expect("create post");

    assert_eq!(post.user_id, user.id);

    // And the relation resolves back through the generated accessor.
    let loaded = Post::filter_by_id(post.id)
        .first()
        .exec(database.toasty_mut())
        .await
        .expect("query")
        .expect("post exists");
    assert_eq!(loaded.title, "Hello");
    assert_eq!(loaded.user_id, user.id);
}

#[tokio::test]
async fn a_nullable_relation_accepts_a_missing_parent() {
    let mut database = database().await;

    let user = User::create()
        .name("Grace")
        .email("grace@example.com")
        .exec(database.toasty_mut())
        .await
        .expect("create user");
    let post = Post::create()
        .title("Draft")
        .body("...")
        .user_id(user.id)
        .exec(database.toasty_mut())
        .await
        .expect("create post");

    // `author` is `Deferred<Option<User>>`, so `author_id` is nullable and may
    // simply be left out.
    let anonymous = Comment::create()
        .body("nice post")
        .post_id(post.id)
        .exec(database.toasty_mut())
        .await
        .expect("anonymous comment");
    assert_eq!(anonymous.author_id, None);

    let signed = Comment::create()
        .body("thanks")
        .post_id(post.id)
        .author_id(Some(user.id))
        .exec(database.toasty_mut())
        .await
        .expect("signed comment");
    assert_eq!(signed.author_id, Some(user.id));
}

#[tokio::test]
async fn seeding_fills_a_parent_and_its_children() {
    let mut database = database().await;
    let mut seeder = Seeder::new(&mut database)
        .expect("tests are not a protected environment")
        .seeded(2026);

    let users = seeder.create::<User>(10).await.expect("seed users");
    assert_eq!(users.len(), 10);
    // The unique-email generator is counter-based, so a batch cannot collide
    // on a unique column.
    let emails: std::collections::HashSet<&str> =
        users.iter().map(|user| user.email.as_str()).collect();
    assert_eq!(emails.len(), 10);

    for user in &users {
        let posts = seeder
            .create_with::<Post, _>(3, user.id)
            .await
            .expect("seed posts");
        assert_eq!(posts.len(), 3);
        assert!(posts.iter().all(|post| post.user_id == user.id));
        assert!(posts.iter().all(|post| !post.title.is_empty()));
    }

    let all = Post::all()
        .exec(database.toasty_mut())
        .await
        .expect("query posts");
    assert_eq!(all.len(), 30);
}

#[tokio::test]
async fn a_fixed_seed_reproduces_the_same_rows() {
    // Replaying a failing fixture is the whole reason seeding is seedable.
    let mut first = database().await;
    let mut second = database().await;

    let left = Seeder::new(&mut first)
        .expect("seeder")
        .seeded(7)
        .create::<User>(5)
        .await
        .expect("seed");
    let right = Seeder::new(&mut second)
        .expect("seeder")
        .seeded(7)
        .create::<User>(5)
        .await
        .expect("seed");

    let names = |users: &[User]| users.iter().map(|u| u.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&left), names(&right));

    let other = Seeder::new(&mut first)
        .expect("seeder")
        .seeded(8)
        .create::<User>(5)
        .await
        .expect("seed");
    assert_ne!(names(&left), names(&other), "a different seed differs");
}

#[tokio::test]
async fn the_locale_changes_generated_names() {
    let mut database = database().await;
    let users = Seeder::new(&mut database)
        .expect("seeder")
        .seeded(3)
        .locale(Locale::ZhCn)
        .create::<User>(5)
        .await
        .expect("seed");

    assert!(
        users.iter().all(|user| !user.name.is_ascii()),
        "zh-CN names are Chinese: {:?}",
        users.iter().map(|u| &u.name).collect::<Vec<_>>()
    );
    assert!(
        users.iter().all(|user| user.email.is_ascii()),
        "but emails stay ASCII, because the column usually is"
    );
}

#[test]
fn the_faker_is_reachable_on_its_own() {
    // For the one-off value between inserts, without a factory.
    let mut faker = Faker::with_seed(1);
    assert!(!faker.name().is_empty());
    assert!(faker.unique_email().contains('@'));
    assert!((1..=6).contains(&faker.int_between(1, 6)));
}
