//! Backend der Read-it-later-App.

pub mod auth;
pub mod config;
pub mod db;
pub mod error;

use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::Method,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use sqlx::SqlitePool;

use crate::{config::Config, error::ApiError};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
}

/// Baut den Router. Alle Routen liegen unter `/api`.
pub fn app(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/register", post(auth::register))
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/account", delete(auth::delete_account))
        .layer(middleware::from_fn(csrf_guard))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state);
    Router::new().nest("/api", api)
}

async fn health() -> &'static str {
    "ok"
}

/// CSRF-Schutz: Zustandsändernde Anfragen müssen einen eigenen Header tragen.
/// Browser senden Custom-Header bei Cross-Site-Anfragen nur nach einem
/// CORS-Preflight, den dieses Backend nie freigibt. Zusammen mit
/// `SameSite=Lax` am Session-Cookie reicht das.
async fn csrf_guard(request: Request, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !safe && !request.headers().contains_key("x-requested-with") {
        return ApiError::Forbidden("Header X-Requested-With fehlt".into()).into_response();
    }
    next.run(request).await
}
