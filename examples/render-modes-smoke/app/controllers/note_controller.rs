use phoenix::database::create;
use phoenix::prelude::{
    Database, Json, Request, Response, State, StatusCode, Validated,
};

use crate::controllers::respond_with_renderer;
use crate::models::Note;
use crate::props::notes::NotesIndexProps;
use crate::requests::StoreNoteRequest;
use crate::resources::NoteResource;

pub struct NoteController;

impl NoteController {
    pub async fn index(request: Request) -> Response {
        let Some(mut db) = request.extensions().get::<Database>().cloned() else {
            return Response::text("database is unavailable")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR);
        };
        match Note::all().exec(db.toasty_mut()).await {
            Ok(notes) => {
                respond_with_renderer(
                    request,
                    phoenix::prelude::Page::new(
                        "notes/index",
                        NotesIndexProps {
                            title: "SQLite notes".to_owned(),
                            notes: notes
                                .into_iter()
                                .map(|note| NoteResource {
                                    id: note.id.to_string(),
                                    name: note.name,
                                })
                                .collect(),
                        },
                    )
                    .islands(),
                )
                .await
            }
            Err(error) => Response::text(format!("failed to list notes: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    #[allow(clippy::unused_async)]
    pub async fn create(_request: Request) -> Response {
        Response::text("POST /notes with JSON {\"name\":\"...\"} to create a note")
    }

    pub async fn store(
        State(mut db): State<Database>,
        Validated(Json(input)): Validated<Json<StoreNoteRequest>>,
    ) -> Result<(StatusCode, Json<NoteResource>), Response> {
        let name = input.name.trim().to_owned();
        match create!(Note { name: name.clone() })
            .exec(db.toasty_mut())
            .await
        {
            Ok(note) => Ok((
                StatusCode::CREATED,
                Json(NoteResource {
                    id: note.id.to_string(),
                    name: note.name,
                }),
            )),
            Err(error) => Err(Response::text(format!("failed to create note: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    #[allow(clippy::unused_async)]
    pub async fn show(_request: Request) -> Response {
        Response::text("NoteController@show")
    }

    #[allow(clippy::unused_async)]
    pub async fn edit(_request: Request) -> Response {
        Response::text("NoteController@edit")
    }

    #[allow(clippy::unused_async)]
    pub async fn update(_request: Request) -> Response {
        Response::text("NoteController@update")
    }

    #[allow(clippy::unused_async)]
    pub async fn destroy(_request: Request) -> Response {
        Response::new(StatusCode::NO_CONTENT, phoenix::http::Bytes::new())
    }
}
