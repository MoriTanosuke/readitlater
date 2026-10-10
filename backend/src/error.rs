//! Einheitlicher Fehlertyp der API (Antwort immer als JSON `{"error": "..."}`).

use axum::{
    Json,
    extract::{FromRequest, rejection::JsonRejection},
    http::{StatusCode, header},
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
    NotFound(String),
    Conflict(String),
    /// Anfrage ist formal korrekt, der Inhalt lässt sich aber nicht verarbeiten.
    Unprocessable(String),
    /// Zu viele gleichzeitige Seitenabrufe.
    TooManyRequests,
    /// Ratenbegrenzung: nach so vielen Sekunden darf es erneut versucht werden.
    RateLimited {
        retry_after_secs: u64,
    },
    /// Die fremde Webseite war nicht erreichbar oder hat nicht geantwortet.
    BadGateway(String),
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
        if let Self::RateLimited { retry_after_secs } = self {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                Json(json!({
                    "error": format!(
                        "Zu viele Anfragen. Bitte in {retry_after_secs} Sekunden erneut versuchen"
                    )
                })),
            )
                .into_response();
        }
        let (status, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Nicht angemeldet".to_string()),
            Self::BadCredentials => (
                StatusCode::UNAUTHORIZED,
                "E-Mail oder Passwort ist falsch".to_string(),
            ),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Unprocessable(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "Es laufen gerade zu viele Abrufe. Bitte gleich noch einmal versuchen".to_string(),
            ),
            // Wird oben behandelt, die Zeile hält nur das match vollständig.
            Self::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "Zu viele Anfragen".to_string(),
            ),
            Self::BadGateway(m) => (StatusCode::BAD_GATEWAY, m),
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
