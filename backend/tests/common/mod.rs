//! Gemeinsame Hilfen für die Integrationstests.
#![allow(dead_code)] // Jede Testdatei nutzt nur einen Teil davon.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use readlater_backend::{AppState, app, config::Config, db};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

pub const PASSWORD: &str = "ein-sicheres-passwort";

pub struct TestApp {
    pub router: Router,
    pub pool: SqlitePool,
}

pub fn test_config() -> Config {
    Config {
        database_path: ":memory:".into(),
        bind_addr: "127.0.0.1:0".into(),
        ..Config::default()
    }
}

pub async fn setup(registration_enabled: bool) -> TestApp {
    setup_with(Config {
        registration_enabled,
        ..test_config()
    })
    .await
}

pub async fn setup_with(config: Config) -> TestApp {
    let pool = db::connect_memory().await.expect("DB");
    let state = AppState::new(pool.clone(), config).expect("AppState");
    TestApp {
        router: app(state),
        pool,
    }
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
}

impl Reply {
    /// `name=wert` aus dem ersten Set-Cookie-Header.
    pub fn cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").to_string())
    }

    pub fn set_cookie_raw(&self) -> String {
        self.headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// Fehlermeldung aus `{"error": "..."}` (leer, wenn keine vorhanden).
    pub fn error(&self) -> String {
        self.body["error"].as_str().unwrap_or("").to_string()
    }
}

pub async fn call(
    app: &TestApp,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    with_csrf_header: bool,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(uri);
    if with_csrf_header {
        builder = builder.header("x-requested-with", "test");
    }
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        body,
    }
}

pub async fn post(app: &TestApp, uri: &str, body: Value, cookie: Option<&str>) -> Reply {
    call(app, Method::POST, uri, Some(body), cookie, true).await
}

pub async fn get(app: &TestApp, uri: &str, cookie: Option<&str>) -> Reply {
    call(app, Method::GET, uri, None, cookie, false).await
}

pub async fn delete(app: &TestApp, uri: &str, cookie: Option<&str>) -> Reply {
    call(app, Method::DELETE, uri, None, cookie, true).await
}

pub async fn register(app: &TestApp, email: &str) -> Reply {
    post(
        app,
        "/api/register",
        json!({ "email": email, "password": PASSWORD }),
        None,
    )
    .await
}

/// Registriert einen Benutzer und gibt dessen Session-Cookie zurück.
pub async fn register_cookie(app: &TestApp, email: &str) -> String {
    register(app, email).await.cookie().expect("Cookie")
}

pub async fn count(pool: &SqlitePool, table: &str) -> i64 {
    let sql: &'static str = match table {
        "users" => "SELECT COUNT(*) FROM users",
        "sessions" => "SELECT COUNT(*) FROM sessions",
        "articles" => "SELECT COUNT(*) FROM articles",
        "tags" => "SELECT COUNT(*) FROM tags",
        "article_tags" => "SELECT COUNT(*) FROM article_tags",
        other => panic!("unbekannte Tabelle: {other}"),
    };
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}
