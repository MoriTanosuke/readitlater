//! Integrationstests für die Leseposition (`read_pos`).
//!
//! Die Artikel werden direkt in die Datenbank geschrieben, damit kein Webserver nötig ist.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

async fn seed(app: &TestApp, email: &str, url: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES ((SELECT id FROM users WHERE email = ?), ?, 'Titel', \
                 '<p>Ein langer Text.</p>', 'Ein langer Text.', '') RETURNING id",
    )
    .bind(email)
    .bind(url)
    .fetch_one(&app.pool)
    .await
    .unwrap()
}

async fn patch(app: &TestApp, cookie: &str, id: i64, body: Value) -> Reply {
    call(
        app,
        Method::PATCH,
        &format!("/api/articles/{id}"),
        Some(body),
        Some(cookie),
        true,
    )
    .await
}

async fn show(app: &TestApp, cookie: &str, id: i64) -> Value {
    let reply = get(app, &format!("/api/articles/{id}"), Some(cookie)).await;
    assert_eq!(reply.status, StatusCode::OK);
    reply.body
}

#[tokio::test]
async fn neue_artikel_beginnen_am_anfang_und_die_position_bleibt_erhalten() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 0);

    let reply = patch(&app, &cookie, id, json!({ "read_pos": 1234 })).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 1234);

    // Die nächste Position ersetzt die vorige, ein Zurückscrollen ist erlaubt.
    patch(&app, &cookie, id, json!({ "read_pos": 40 })).await;
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 40);
}

#[tokio::test]
async fn position_aendert_weder_status_noch_schlagwoerter() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    patch(&app, &cookie, id, json!({ "tags": ["rust", "web"] })).await;

    let reply = patch(&app, &cookie, id, json!({ "read_pos": 500 })).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["is_read"], false);
    assert_eq!(reply.body["tags"], json!(["rust", "web"]));

    // Schlagwörter und Status ändern die Position nicht.
    patch(&app, &cookie, id, json!({ "tags": ["neu"] })).await;
    patch(&app, &cookie, id, json!({ "is_read": false })).await;
    patch(&app, &cookie, id, json!({})).await;
    let article = show(&app, &cookie, id).await;
    assert_eq!(article["read_pos"], 500);
    assert_eq!(article["tags"], json!(["neu"]));
}

#[tokio::test]
async fn gelesen_markieren_setzt_die_position_zurueck() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    patch(&app, &cookie, id, json!({ "read_pos": 900 })).await;

    let reply = patch(&app, &cookie, id, json!({ "is_read": true })).await;
    assert_eq!(reply.body["is_read"], true);
    let article = show(&app, &cookie, id).await;
    assert_eq!(article["is_read"], true);
    assert_eq!(article["read_pos"], 0, "gelesene Artikel starten oben");

    // Wieder ungelesen: die Position bleibt 0, bis erneut gelesen wird.
    patch(&app, &cookie, id, json!({ "is_read": false })).await;
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 0);
    patch(&app, &cookie, id, json!({ "read_pos": 77 })).await;
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 77);
}

#[tokio::test]
async fn bei_gelesenen_artikeln_wird_keine_position_gemerkt() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    patch(&app, &cookie, id, json!({ "is_read": true })).await;

    // Eine verspätete Positions-Anfrage nach „gelesen“ bringt die Position nicht zurück.
    let spaet = patch(&app, &cookie, id, json!({ "read_pos": 300 })).await;
    assert_eq!(spaet.status, StatusCode::OK);
    assert_eq!(show(&app, &cookie, id).await["read_pos"], 0);

    // Beides in einer Anfrage: „gelesen“ gewinnt.
    patch(&app, &cookie, id, json!({ "is_read": false })).await;
    patch(&app, &cookie, id, json!({ "read_pos": 300 })).await;
    patch(
        &app,
        &cookie,
        id,
        json!({ "is_read": true, "read_pos": 300 }),
    )
    .await;
    let article = show(&app, &cookie, id).await;
    assert_eq!(article["is_read"], true);
    assert_eq!(article["read_pos"], 0);

    // Zurück auf ungelesen mit neuer Position in einer Anfrage: die Position gilt.
    patch(
        &app,
        &cookie,
        id,
        json!({ "is_read": false, "read_pos": 12 }),
    )
    .await;
    let article = show(&app, &cookie, id).await;
    assert_eq!(article["is_read"], false);
    assert_eq!(article["read_pos"], 12);
}

#[tokio::test]
async fn ungueltige_positionen_werden_abgelehnt_ohne_etwas_zu_aendern() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    patch(&app, &cookie, id, json!({ "read_pos": 55 })).await;

    for body in [
        json!({ "read_pos": -1 }),
        json!({ "read_pos": 10_000_001 }),
        json!({ "read_pos": 1.5 }),
        json!({ "read_pos": "viel" }),
        json!({ "read_pos": 99999999999999999999.0 }),
        // Eine ungültige Position verhindert auch die anderen Änderungen derselben Anfrage.
        json!({ "read_pos": -5, "is_read": true }),
    ] {
        let reply = patch(&app, &cookie, id, body.clone()).await;
        assert!(reply.status.is_client_error(), "{body}: {}", reply.status);
    }
    let article = show(&app, &cookie, id).await;
    assert_eq!(article["read_pos"], 55);
    assert_eq!(article["is_read"], false);

    // Die Grenzen selbst sind gültig.
    for pos in [0, 10_000_000] {
        let reply = patch(&app, &cookie, id, json!({ "read_pos": pos })).await;
        assert_eq!(reply.status, StatusCode::OK, "{pos}");
        assert_eq!(show(&app, &cookie, id).await["read_pos"], pos);
    }
}

#[tokio::test]
async fn fremde_artikel_haben_eine_eigene_position() {
    let app = setup(true).await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;
    let anna_art = seed(&app, "anna@example.com", "https://a.example/1").await;
    let bert_art = seed(&app, "bert@example.com", "https://a.example/1").await;

    patch(&app, &anna, anna_art, json!({ "read_pos": 100 })).await;
    patch(&app, &bert, bert_art, json!({ "read_pos": 200 })).await;
    assert_eq!(show(&app, &anna, anna_art).await["read_pos"], 100);
    assert_eq!(show(&app, &bert, bert_art).await["read_pos"], 200);

    // Fremde Artikel lassen sich weder ändern noch sehen.
    let fremd = patch(&app, &anna, bert_art, json!({ "read_pos": 1 })).await;
    assert_eq!(fremd.status, StatusCode::NOT_FOUND);
    assert_eq!(show(&app, &bert, bert_art).await["read_pos"], 200);
    let nicht_sichtbar = get(&app, &format!("/api/articles/{bert_art}"), Some(&anna)).await;
    assert_eq!(nicht_sichtbar.status, StatusCode::NOT_FOUND);

    // Ohne Login geht nichts.
    let ohne_login = call(
        &app,
        Method::PATCH,
        &format!("/api/articles/{anna_art}"),
        Some(json!({ "read_pos": 5 })),
        None,
        true,
    )
    .await;
    assert_eq!(ohne_login.status, StatusCode::UNAUTHORIZED);
    assert_eq!(show(&app, &anna, anna_art).await["read_pos"], 100);
}

#[tokio::test]
async fn die_position_steht_nicht_in_der_liste_und_nicht_im_export() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let id = seed(&app, "anna@example.com", "https://a.example/1").await;
    patch(&app, &cookie, id, json!({ "read_pos": 321 })).await;

    // Die Liste bleibt schlank: Die Position braucht nur der Lesemodus.
    let list = get(&app, "/api/articles", Some(&cookie)).await;
    assert!(list.body[0].get("read_pos").is_none(), "{:?}", list.body);

    // Die Position ist flüchtiger Zustand und gehört nicht in die Exportdatei.
    let export = get(&app, "/api/export", Some(&cookie)).await;
    assert_eq!(export.status, StatusCode::OK);
    assert!(
        !export.body.to_string().contains("read_pos"),
        "{:?}",
        export.body
    );
}
