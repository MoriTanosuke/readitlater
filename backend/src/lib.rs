//! Backend der Read-it-later-App.

pub mod articles;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod extract;
pub mod fetch;
pub mod ratelimit;
pub mod share;
pub mod tags;

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::Method,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::{config::Config, error::ApiError, fetch::Fetcher};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// HTTP-Client zum Laden fremder Seiten (mit SSRF-Schutz).
    pub fetcher: Arc<Fetcher>,
    /// Begrenzt die gleichzeitigen Seitenabrufe.
    pub fetch_slots: Arc<Semaphore>,
    /// Ratenbegrenzung (nur im Speicher).
    pub limits: Arc<ratelimit::Limits>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Config) -> Result<Self, reqwest::Error> {
        let fetcher = Fetcher::new(
            config.fetch_allow_private,
            Duration::from_secs(config.fetch_timeout_secs),
            config.fetch_max_bytes,
        )?;
        Ok(Self {
            pool,
            fetcher: Arc::new(fetcher),
            fetch_slots: Arc::new(Semaphore::new(config.fetch_concurrency)),
            limits: Arc::new(ratelimit::Limits::new(config.rate_limits)),
            config: Arc::new(config),
        })
    }
}

/// Baut den Router. Alle Routen liegen unter `/api`.
pub fn app(state: AppState) -> Router {
    // Routen, die Passwörter prüfen oder hashen: strengere Ratenbegrenzung pro IP.
    let password_routes = Router::new()
        .route("/register", post(auth::register))
        .route("/login", post(auth::login))
        .route("/account", delete(auth::delete_account))
        .route("/account/password", put(auth::change_password))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            ratelimit::auth,
        ));

    let api = Router::new()
        .route("/health", get(health))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .merge(password_routes)
        .route("/articles", post(articles::create).get(articles::list))
        .route(
            "/articles/{id}",
            get(articles::show)
                .patch(articles::update)
                .delete(articles::remove),
        )
        .route("/tags", get(tags::list))
        .route(
            "/share-tokens",
            get(share::list_tokens).post(share::create_token),
        )
        .route("/share-tokens/{id}", delete(share::delete_token))
        .layer(middleware::from_fn(csrf_guard))
        // Teilen per Token: nutzt nur den Header Authorization und kein Cookie,
        // darum ohne CSRF-Header (HTTP Shortcuts und Kurzbefehle setzen ihn nicht).
        .route("/share", post(share::share))
        .layer(DefaultBodyLimit::max(64 * 1024))
        // Äußerste Schicht: Client-Adresse bestimmen und allgemeines Limit anwenden,
        // bevor Body, Anmeldung oder Hashing etwas kosten.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            ratelimit::general,
        ))
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
