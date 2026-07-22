use phoenix_database::{Database, Deferred, Model, TestDatabase, create, models};

#[derive(Debug, Model)]
struct Author {
    #[key]
    #[auto]
    id: u64,
    name: String,
    #[has_many]
    posts: Deferred<Vec<Post>>,
}

#[derive(Debug, Model)]
struct Post {
    #[key]
    #[auto]
    id: u64,
    #[index]
    author_id: u64,
    #[belongs_to]
    author: Deferred<Author>,
    title: String,
}

async fn exercise_database(mut database: Database) {
    database.initialize_schema().await.unwrap();

    let mut author = create!(Author {
        name: "Ada",
        posts: [{ title: "First" }, { title: "Second" }],
    })
    .exec(database.toasty_mut())
    .await
    .unwrap();

    let loaded = Author::filter_by_id(author.id)
        .get(database.toasty_mut())
        .await
        .unwrap();
    assert_eq!(loaded.name, "Ada");
    let posts = loaded.posts().exec(database.toasty_mut()).await.unwrap();
    assert_eq!(posts.len(), 2);
    assert_eq!(
        posts[0]
            .author()
            .exec(database.toasty_mut())
            .await
            .unwrap()
            .id,
        author.id
    );

    author
        .update()
        .name("Grace")
        .exec(database.toasty_mut())
        .await
        .unwrap();
    assert_eq!(author.name, "Grace");

    for index in 0..5 {
        Author::create()
            .name(format!("Author {index}"))
            .exec(database.toasty_mut())
            .await
            .unwrap();
    }
    let first_page = Author::all()
        .order_by(Author::fields().id().asc())
        .paginate(2)
        .exec(database.toasty_mut())
        .await
        .unwrap();
    assert_eq!(first_page.len(), 2);
    assert!(first_page.has_next());
    let second_page = first_page
        .next(database.toasty_mut())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_page.len(), 2);
    assert_ne!(first_page[0].id, second_page[0].id);

    let count_before_delete = Author::all()
        .count()
        .exec(database.toasty_mut())
        .await
        .unwrap();
    author.delete().exec(database.toasty_mut()).await.unwrap();
    assert_eq!(
        Author::all()
            .count()
            .exec(database.toasty_mut())
            .await
            .unwrap(),
        count_before_delete - 1
    );
}

#[tokio::test]
async fn sqlite_crud_relations_and_pagination() {
    let database = Database::sqlite_memory(models!(Author)).await.unwrap();
    exercise_database(database).await;
}

#[tokio::test]
async fn transactions_commit_and_roll_back() {
    let mut fixture = TestDatabase::new(models!(Author)).await.unwrap();

    {
        let mut transaction = fixture.toasty_mut().transaction().await.unwrap();
        Author::create()
            .name("Rolled back")
            .exec(&mut transaction)
            .await
            .unwrap();
        transaction.rollback().await.unwrap();
    }
    assert_eq!(
        Author::all()
            .count()
            .exec(fixture.toasty_mut())
            .await
            .unwrap(),
        0
    );

    {
        let mut transaction = fixture.toasty_mut().transaction().await.unwrap();
        Author::create()
            .name("Committed")
            .exec(&mut transaction)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    }
    assert_eq!(
        Author::all()
            .count()
            .exec(fixture.toasty_mut())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn postgresql_crud_relations_and_pagination_when_configured() {
    let Ok(url) = std::env::var("PHOENIX_TEST_POSTGRES_URL") else {
        return;
    };
    let database = Database::builder(models!(Author))
        .table_prefix("phoenix_contract_")
        .connect(&url)
        .await
        .unwrap();
    exercise_database(database).await;
}
