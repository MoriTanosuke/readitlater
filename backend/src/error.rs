//! Einheitlicher Fehlertyp der API (Antwort immer als JSON `{"error": "..."}`).

use axum::{
    Json,
    extract::{FromRequest, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    /// Falsche E-Mail oder falsches Passwort (bewusst nicht unterscheidbar).
    BadCredentials,
    Forbidden(String),
    Conflict(String),
    Internal(String),
}

impl ApiError {
    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::internal(err)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::BadRequest(format!("Ungültige Anfrage: {}", rejection.body_text()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Nicht angemeldet".to_string()),
            Self::BadCredentials => (
                StatusCode::UNAUTHORIZED,
                "E-Mail oder Passwort ist falsch".to_string(),
            ),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Internal(detail) => {
                // Details nur ins Log, nie an den Client.
                tracing::error!("interner Fehler: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Interner Serverfehler".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// JSON-Extraktor, der Fehler im API-Format zurückgibt.
#[derive(FromRequest)]
#[from_request(via(Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);
