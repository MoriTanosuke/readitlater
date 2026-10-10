//! Integrationstests für das Teilen per Token (`POST /api/share`) und die Token-Verwaltung.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Html,
    routing::get as route_get,
};
use common::*;
use http_body_util::BodyExt;
use readlater_backend::config::Config;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

fn page(title: &str) -> String {
    let absatz = "<p>Dies ist ein ausführlicher Absatz mit genug Text, damit die Extraktion ihn \
                  sicher als Hauptinhalt der Seite erkennt und nicht als Beiwerk verwirft.</p>";
    format!(
        "<!doctype html><html><head><title>{title}</title></head><body><article>{}</article>\
         </body></html>",
        absatz.repeat(6)
    )
}

async fn spawn_site() -> String {
    let router = Router::new()
        .route(
            "/a",
            route_get(|| async { Html(page("Geteilter Artikel")) }),
        )
        .route("/b", route_get(|| async { Html(page("Zweiter Artikel")) }))
        .route("/c", route_get(|| async { Html(page("Dritter Artikel")) }))
        .route("/d", route_get(|| async { Html(page("Vierter Artikel")) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn app() -> TestApp {
    setup_with(Config {
        fetch_allow_private: true,
        fetch_timeout_secs: 2,
        ..test_config()
    })
    .await
}

/// Legt ein Token an und gibt dessen Wert zurück.
async fn new_token(app: &TestApp, cookie: &str, name: &str) -> String {
    let reply = post(
        app,
        "/api/share-tokens",
        json!({ "name": name }),
        Some(cookie),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply.body);
    reply.body["token"].as_str().unwrap().to_owned()
}

/// Anfrage wie von HTTP Shortcuts oder den Kurzbefehlen: nur `Authorization`, kein
/// Cookie und kein `X-Requested-With`.
async fn share_raw(
    app: &TestApp,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(Method::POST).uri("/api/share");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    let response = app
        .router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn teilen_per_json_speichert_den_artikel_ohne_cookie() {
    let app = app().await;
    let site = spawn_site().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "Pixel").await;

    let (status, body) = share_raw(
        &app,
        Some(&token),
        Some("application/json"),
        &json!({ "url": format!("{site}/a") }).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert_eq!(body["title"], "Geteilter Artikel");

    // Der Artikel gehört dem Benutzer, der das Token angelegt hat.
    let list = get(&app, "/api/articles", Some(&cookie)).await;
    assert_eq!(list.body.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn teilen_akzeptiert_formular_und_reinen_text_und_android_freitext() {
    let app = app().await;
    let site = spawn_site().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "iPad").await;

    let form = format!(
        "url={}",
        format!("{site}/a").replace(':', "%3A").replace('/', "%2F")
    );
    let (s1, _) = share_raw(
        &app,
        Some(&token),
        Some("application/x-www-form-urlencoded"),
        &form,
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED);

    let (s2, _) = share_raw(
        &app,
        Some(&token),
        Some("text/plain"),
        &format!("{site}/b\n"),
    )
    .await;
    assert_eq!(s2, StatusCode::CREATED);

    // Android liefert beim Teilen häufig "Titel Adresse".
    let text = json!({ "text": format!("Lesenswert: {site}/c danke") }).to_string();
    let (s3, _) = share_raw(&app, Some(&token), Some("application/json"), &text).await;
    assert_eq!(s3, StatusCode::CREATED);
    assert_eq!(count(&app.pool, "articles").await, 3);
}

#[tokio::test]
async fn ohne_gueltiges_token_wird_nichts_gespeichert() {
    let app = app().await;
    let site = spawn_site().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let _ = new_token(&app, &cookie, "Pixel").await;
    let body = json!({ "url": format!("{site}/a") }).to_string();

    for token in [None, Some("lsl_falsch"), Some("")] {
        let (status, _) = share_raw(&app, token, Some("application/json"), &body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
    }
    // Das Session-Cookie ersetzt kein Token (und umgekehrt).
    let reply = call(
        &app,
        Method::POST,
        "/api/share",
        Some(json!({ "url": format!("{site}/a") })),
        Some(&cookie),
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(count(&app.pool, "articles").await, 0);
}

#[tokio::test]
async fn token_gilt_nur_fuer_share_und_nicht_fuer_den_rest_der_api() {
    let app = app().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "Pixel").await;
    let bearer = format!("Bearer {token}");

    for (method, uri) in [
        (Method::GET, "/api/articles"),
        (Method::GET, "/api/me"),
        (Method::GET, "/api/share-tokens"),
        (Method::GET, "/api/tags"),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::AUTHORIZATION, &bearer)
                    .header("x-requested-with", "test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn doppelte_und_ungueltige_adressen_liefern_klare_fehler() {
    let app = app().await;
    let site = spawn_site().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "Pixel").await;
    let body = json!({ "url": format!("{site}/a") }).to_string();

    let (first, _) = share_raw(&app, Some(&token), Some("application/json"), &body).await;
    assert_eq!(first, StatusCode::CREATED);
    let (second, _) = share_raw(&app, Some(&token), Some("application/json"), &body).await;
    assert_eq!(second, StatusCode::CONFLICT);

    for (ct, b) in [
        (Some("application/json"), "{}"),
        (Some("application/json"), "kein json"),
        (Some("text/plain"), "   "),
        (None, ""),
    ] {
        let (status, _) = share_raw(&app, Some(&token), ct, b).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{b:?}");
    }
    let (status, _) = share_raw(
        &app,
        Some(&token),
        Some("text/plain"),
        "ftp://example.org/x",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ssrf_schutz_gilt_auch_beim_teilen() {
    let app = setup(true).await; // strenger Modus, localhost gesperrt
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "Pixel").await;
    let (status, _) = share_raw(
        &app,
        Some(&token),
        Some("text/plain"),
        "http://127.0.0.1:1/x",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&app.pool, "articles").await, 0);
}

#[tokio::test]
async fn token_wird_nur_als_hash_gespeichert_und_nur_einmal_angezeigt() {
    let app = app().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "  Mein   Pixel ").await;
    assert!(token.starts_with("lsl_") && token.len() == 4 + 64);

    let stored: Vec<(String, String)> = sqlx::query_as("SELECT name, token_hash FROM share_tokens")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, "Mein Pixel");
    assert_ne!(stored[0].1, token);
    assert!(!stored[0].1.contains(&token));

    let list = get(&app, "/api/share-tokens", Some(&cookie)).await;
    assert_eq!(list.status, StatusCode::OK);
    let text = list.body.to_string();
    assert!(!text.contains(&token) && !text.contains("token_hash"));
    assert_eq!(list.body[0]["name"], "Mein Pixel");
    assert!(list.body[0]["last_used_at"].is_null());
}

#[tokio::test]
async fn zuletzt_benutzt_wird_gesetzt_und_widerruf_sperrt_sofort() {
    let app = app().await;
    let site = spawn_site().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let token = new_token(&app, &cookie, "Pixel").await;

    let (status, _) = share_raw(&app, Some(&token), Some("text/plain"), &format!("{site}/a")).await;
    assert_eq!(status, StatusCode::CREATED);
    let list = get(&app, "/api/share-tokens", Some(&cookie)).await;
    assert!(list.body[0]["last_used_at"].is_string());

    let id = list.body[0]["id"].as_i64().unwrap();
    let deleted = delete(&app, &format!("/api/share-tokens/{id}"), Some(&cookie)).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let (status, _) = share_raw(&app, Some(&token), Some("text/plain"), &format!("{site}/b")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tokens_sind_pro_benutzer_getrennt() {
    let app = app().await;
    let site = spawn_site().await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let token_a = new_token(&app, &a, "A").await;

    // B sieht und löscht das Token von A nicht.
    assert_eq!(
        get(&app, "/api/share-tokens", Some(&b)).await.body,
        json!([])
    );
    let id: i64 = sqlx::query_scalar("SELECT id FROM share_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let reply = delete(&app, &format!("/api/share-tokens/{id}"), Some(&b)).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);

    // Artikel landen beim Besitzer des Tokens.
    let (status, _) = share_raw(
        &app,
        Some(&token_a),
        Some("text/plain"),
        &format!("{site}/a"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(get(&app, "/api/articles", Some(&b)).await.body, json!([]));
}

#[tokio::test]
async fn verwaltung_braucht_login_csrf_header_und_gueltigen_namen() {
    let app = app().await;
    let cookie = register_cookie(&app, "a@example.org").await;

    let anon = post(&app, "/api/share-tokens", json!({ "name": "x" }), None).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    let no_csrf = call(
        &app,
        Method::POST,
        "/api/share-tokens",
        Some(json!({ "name": "x" })),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);

    for name in ["", "   ", &"x".repeat(61), "a\u{7}b"] {
        let reply = post(
            &app,
            "/api/share-tokens",
            json!({ "name": name }),
            Some(&cookie),
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{name:?}");
    }
}

#[tokio::test]
async fn hoechstens_zehn_tokens_und_konto_loeschen_entfernt_sie() {
    let app = app().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    for i in 0..10 {
        new_token(&app, &cookie, &format!("Gerät {i}")).await;
    }
    let eleventh = post(
        &app,
        "/api/share-tokens",
        json!({ "name": "zu viel" }),
        Some(&cookie),
    )
    .await;
    assert_eq!(eleventh.status, StatusCode::UNPROCESSABLE_ENTITY);

    let reply = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&cookie),
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM share_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
