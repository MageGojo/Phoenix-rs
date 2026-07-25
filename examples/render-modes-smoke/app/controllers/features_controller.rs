use std::sync::atomic::Ordering;
use std::time::Duration;

use futures_util::stream;
use phoenix::http::Bytes;
use phoenix::prelude::{
    CurrentPrincipal, EmailMessage, IntoResponse, Json, Jwt, KeepAlive, Password, Query, Request,
    Response, Sse, SseEvent, State, StatusCode, Storage, WebSocketUpgrade,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::features::FeatureServices;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SmokeClaims {
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PasswordBody {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyPasswordBody {
    pub password: String,
    pub hash: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueTokenBody {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct StoragePutBody {
    pub key: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct StorageGetQuery {
    pub key: String,
}

pub struct FeaturesController;

impl FeaturesController {
    pub async fn sse(_request: Request) -> impl IntoResponse {
        Sse::from_events(stream::iter([SseEvent::new().data("hello")]))
            .keep_alive(KeepAlive::new(Duration::from_secs(15)).expect("keep-alive"))
    }

    pub async fn ws(ws: WebSocketUpgrade) -> Response {
        ws.any_origin().on_upgrade(|mut socket| async move {
            while let Some(Ok(message)) = socket.recv().await {
                if !message.is_text() {
                    continue;
                }
                let Ok(text) = message.into_text() else {
                    break;
                };
                let reply = if text == "ping" {
                    "pong".to_owned()
                } else {
                    text
                };
                if socket
                    .send(phoenix::prelude::Message::text(reply))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    pub async fn password_hash(Json(body): Json<PasswordBody>) -> Response {
        match Password::hash(&body.password) {
            Ok(hash) => Json(json!({ "hash": hash })).into_response(),
            Err(error) => Response::text(format!("hash failed: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    pub async fn password_verify(Json(body): Json<VerifyPasswordBody>) -> Response {
        match Password::verify(&body.password, &body.hash) {
            Ok(ok) => Json(json!({ "ok": ok })).into_response(),
            Err(error) => Response::text(format!("verify failed: {error}"))
                .with_status(StatusCode::BAD_REQUEST),
        }
    }

    pub async fn jwt_token(
        State(services): State<FeatureServices>,
        Json(body): Json<IssueTokenBody>,
    ) -> Response {
        let role = body.role.trim();
        if role.is_empty() {
            return Response::text("role is required").with_status(StatusCode::BAD_REQUEST);
        }
        match services.jwt.issue(
            "smoke-user",
            SmokeClaims {
                roles: vec![role.to_owned()],
            },
        ) {
            Ok(token) => Json(json!({ "token": token, "role": role })).into_response(),
            Err(error) => Response::text(format!("issue failed: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    pub async fn jwt_me(claims: Jwt<SmokeClaims>) -> Json<serde_json::Value> {
        Json(json!({
            "sub": claims.sub,
            "roles": claims.custom.roles,
        }))
    }

    pub async fn admin(principal: CurrentPrincipal) -> Json<serde_json::Value> {
        Json(json!({
            "ok": true,
            "subject": principal.subject(),
        }))
    }

    pub async fn storage_put(
        State(services): State<FeatureServices>,
        Json(body): Json<StoragePutBody>,
    ) -> Response {
        match services
            .storage
            .put(&body.key, Bytes::from(body.content.into_bytes()))
            .await
        {
            Ok(()) => Json(json!({ "stored": body.key })).into_response(),
            Err(error) => Response::text(format!("storage put failed: {error}"))
                .with_status(StatusCode::BAD_REQUEST),
        }
    }

    pub async fn storage_get(
        State(services): State<FeatureServices>,
        Query(query): Query<StorageGetQuery>,
    ) -> Response {
        match services.storage.get(&query.key).await {
            Ok(bytes) => Response::text(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => Response::text(format!("storage get failed: {error}"))
                .with_status(StatusCode::NOT_FOUND),
        }
    }

    pub async fn queue_ping(State(services): State<FeatureServices>) -> Response {
        let before = services.queue_acked.load(Ordering::SeqCst);
        if let Err(error) = services.queue.dispatch("ping", json!({})).await {
            return Response::text(format!("queue dispatch failed: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR);
        }
        for _ in 0..50 {
            if services.queue_acked.load(Ordering::SeqCst) > before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let acked = services.queue_acked.load(Ordering::SeqCst);
        Json(json!({ "acked": acked, "before": before })).into_response()
    }

    pub async fn mail_send(State(services): State<FeatureServices>) -> Response {
        let email = match EmailMessage::builder()
            .from("noreply@example.com")
            .to("user@example.com")
            .subject("smoke")
            .text_body("hello from render-modes-smoke")
            .build()
        {
            Ok(email) => email,
            Err(error) => {
                return Response::text(format!("mail build failed: {error}"))
                    .with_status(StatusCode::BAD_REQUEST);
            }
        };
        if let Err(error) = services.mailer.send(email).await {
            return Response::text(format!("mail send failed: {error}"))
                .with_status(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Json(json!({ "sent": services.mail_sent.len() })).into_response()
    }

    pub async fn mail_sent(State(services): State<FeatureServices>) -> Response {
        Json(json!({ "count": services.mail_sent.len() })).into_response()
    }
}
