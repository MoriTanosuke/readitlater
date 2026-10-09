//! Integrationstests für Registrierung, Login, Logout und Konto löschen.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn health_ist_ohne_login_erreichbar() {
    let app = setup(true).await;
    let reply = get(&app, "/api/health", None).await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn registrieren_meldet_an_und_setzt_sicheres_cookie() {
    let app = setup(true).await;
    let reply = register(&app, "Anna@Example.com").await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.body["email"], "anna@example.com");

    let raw = reply.set_cookie_raw();
    assert!(raw.contains("HttpOnly"), "{raw}");
    assert!(raw.contains("SameSite=Lax"), "{raw}");
    assert!(raw.contains("Path=/"), "{raw}");

    let cookie = reply.cookie().unwrap();
    let me = get(&app, "/api/me", Some(&cookie)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["email"], "anna@example.com");
}

#[tokio::test]
async fn passwort_wird_nur_als_hash_gespeichert() {
    let app = setup(true).await;
    register(&app, "anna@example.com").await;
    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert!(!stored.contains(PASSWORD));
}

#[tokio::test]
async fn doppelte_email_wird_abgelehnt_auch_bei_anderer_schreibweise() {
    let app = setup(true).await;
    assert_eq!(
        register(&app, "anna@example.com").await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        register(&app, "ANNA@example.com").await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn ungueltige_eingaben_werden_abgelehnt() {
    let app = setup(true).await;
    for email in ["keine-mail", "a@b", "@example.com", "a b@example.com", ""] {
        let reply = register(&app, email).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{email}");
    }
    let kurz = post(
        &app,
        "/api/register",
        json!({ "email": "anna@example.com", "password": "zu-kurz" }),
        None,
    )
    .await;
    assert_eq!(kurz.status, StatusCode::BAD_REQUEST);

    let kaputt = call(
        &app,
        Method::POST,
        "/api/register",
        Some(json!({ "email": "anna@example.com" })),
        None,
        true,
    )
    .await;
    assert_eq!(kaputt.status, StatusCode::BAD_REQUEST);
    assert!(kaputt.body["error"].is_string());
}

#[tokio::test]
async fn registrierung_kann_deaktiviert_werden() {
    let app = setup(false).await;
    let reply = register(&app, "anna@example.com").await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app.pool, "users").await, 0);
}

#[tokio::test]
async fn login_funktioniert_und_fehler_sind_nicht_unterscheidbar() {
    let app = setup(true).await;
    register(&app, "anna@example.com").await;

    let ok = post(
        &app,
        "/api/login",
        json!({ "email": "ANNA@example.com", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert!(ok.cookie().is_some());

    let falsches_passwort = post(
        &app,
        "/api/login",
        json!({ "email": "anna@example.com", "password": "ganz-falsch-123" }),
        None,
    )
    .await;
    let unbekannt = post(
        &app,
        "/api/login",
        json!({ "email": "niemand@example.com", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(falsches_passwort.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unbekannt.status, StatusCode::UNAUTHORIZED);
    assert_eq!(falsches_passwort.body, unbekannt.body);
    assert!(falsches_passwort.cookie().is_none());
}

#[tokio::test]
async fn logout_beendet_die_session() {
    let app = setup(true).await;
    let cookie = register(&app, "anna@example.com").await.cookie().unwrap();

    let reply = post(&app, "/api/logout", json!({}), Some(&cookie)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);

    let me = get(&app, "/api/me", Some(&cookie)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    assert_eq!(count(&app.pool, "sessions").await, 0);
}

#[tokio::test]
async fn abgelaufene_session_wird_nicht_akzeptiert() {
    let app = setup(true).await;
    let cookie = register(&app, "anna@example.com").await.cookie().unwrap();
    sqlx::query("UPDATE sessions SET expires_at = 1")
        .execute(&app.pool)
        .await
        .unwrap();
    let me = get(&app, "/api/me", Some(&cookie)).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ohne_login_kein_zugriff() {
    let app = setup(true).await;
    assert_eq!(
        get(&app, "/api/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let reply = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        None,
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn zustandsaendernde_anfragen_brauchen_den_csrf_header() {
    let app = setup(true).await;
    let reply = call(
        &app,
        Method::POST,
        "/api/register",
        Some(json!({ "email": "anna@example.com", "password": PASSWORD })),
        None,
        false,
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app.pool, "users").await, 0);
}

#[tokio::test]
async fn konto_loeschen_entfernt_alle_daten_des_benutzers() {
    let app = setup(true).await;
    let anna = register(&app, "anna@example.com").await;
    let anna_id = anna.body["id"].as_i64().unwrap();
    let anna_cookie = anna.cookie().unwrap();
    let bert = register(&app, "bert@example.com").await;
    let bert_id = bert.body["id"].as_i64().unwrap();
    let bert_cookie = bert.cookie().unwrap();

    // Artikel und Schlagwörter direkt anlegen (Schlagwort-API folgt in Schritt 3).
    for (user, url, tag) in [
        (anna_id, "https://example.com/a", "rust"),
        (bert_id, "https://example.com/b", "web"),
    ] {
        let article_id = sqlx::query("INSERT INTO articles (user_id, url) VALUES (?, ?)")
            .bind(user)
            .bind(url)
            .execute(&app.pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let tag_id = sqlx::query("INSERT INTO tags (user_id, name) VALUES (?, ?)")
            .bind(user)
            .bind(tag)
            .execute(&app.pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query("INSERT INTO article_tags (article_id, tag_id) VALUES (?, ?)")
            .bind(article_id)
            .bind(tag_id)
            .execute(&app.pool)
            .await
            .unwrap();
    }

    // Falsches Passwort: nichts passiert.
    let falsch = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": "falsches-passwort" })),
        Some(&anna_cookie),
        true,
    )
    .await;
    assert_eq!(falsch.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app.pool, "users").await, 2);

    // Richtiges Passwort: Konto weg, Cookie wird entfernt.
    let ok = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&anna_cookie),
        true,
    )
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    assert!(ok.set_cookie_raw().starts_with("session="));

    assert_eq!(count(&app.pool, "users").await, 1);
    assert_eq!(count(&app.pool, "sessions").await, 1);
    assert_eq!(count(&app.pool, "articles").await, 1);
    assert_eq!(count(&app.pool, "tags").await, 1);
    assert_eq!(count(&app.pool, "article_tags").await, 1);

    // Annas Session ist ungültig, Berts Konto unberührt.
    assert_eq!(
        get(&app, "/api/me", Some(&anna_cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/api/me", Some(&bert_cookie)).await.status,
        StatusCode::OK
    );

    // Die E-Mail ist danach wieder frei.
    assert_eq!(
        register(&app, "anna@example.com").await.status,
        StatusCode::CREATED
    );
}
