//! Integrationstests für Markierungen und Notizen.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

async fn seed(app: &TestApp, email: &str, url: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES ((SELECT id FROM users WHERE email = ?), ?, 'Titel', \
                 '<p>Ein Satz zum Markieren.</p>', 'Ein Satz zum Markieren.', '') RETURNING id",
    )
    .bind(email)
    .bind(url)
    .fetch_one(&app.pool)
    .await
    .unwrap()
}

fn new_highlight() -> Value {
    json!({ "start": 4, "end": 8, "quote": "Satz", "prefix": "Ein ", "suffix": " zum",
            "color": "#FFEB3B", "note": "Merken" })
}

async fn create(app: &TestApp, cookie: &str, article: i64, body: Value) -> Reply {
    post(
        app,
        &format!("/api/articles/{article}/highlights"),
        body,
        Some(cookie),
    )
    .await
}

async fn list(app: &TestApp, cookie: &str, article: i64) -> Reply {
    get(
        app,
        &format!("/api/articles/{article}/highlights"),
        Some(cookie),
    )
    .await
}

async fn patch(app: &TestApp, cookie: &str, id: i64, body: Value) -> Reply {
    call(
        app,
        Method::PATCH,
        &format!("/api/highlights/{id}"),
        Some(body),
        Some(cookie),
        true,
    )
    .await
}

#[tokio::test]
async fn anlegen_auflisten_aendern_und_loeschen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let article = seed(&app, "a@example.org", "https://example.org/a").await;

    let r = create(&app, &a, article, new_highlight()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let id = r.body["id"].as_i64().unwrap();
    assert_eq!(r.body["start"], 4);
    assert_eq!(r.body["end"], 8);
    assert_eq!(r.body["quote"], "Satz");
    assert_eq!(r.body["color"], "#ffeb3b", "Farbe wird kleingeschrieben");
    assert_eq!(r.body["note"], "Merken");

    // Ohne Notiz, vor der ersten Markierung einsortiert.
    let r2 = create(
        &app,
        &a,
        article,
        json!({ "start": 0, "end": 3, "quote": "Ein", "color": "#80cbc4" }),
    )
    .await;
    assert_eq!(r2.status, StatusCode::CREATED);
    assert_eq!(r2.body["note"], Value::Null);
    let id2 = r2.body["id"].as_i64().unwrap();

    let l = list(&app, &a, article).await;
    assert_eq!(l.status, StatusCode::OK);
    let ids: Vec<i64> = l
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [id2, id], "sortiert nach Position");

    // Farbe ändern, Notiz bleibt.
    let p = patch(&app, &a, id, json!({ "color": "#00FF00" })).await;
    assert_eq!(p.status, StatusCode::OK, "{:?}", p.body);
    assert_eq!(p.body["color"], "#00ff00");
    assert_eq!(p.body["note"], "Merken");
    // Notiz ändern, Farbe bleibt.
    let p = patch(&app, &a, id, json!({ "note": "  Neu  " })).await;
    assert_eq!(p.body["note"], "Neu");
    assert_eq!(p.body["color"], "#00ff00");
    // Notiz mit null oder leer löschen.
    let p = patch(&app, &a, id, json!({ "note": null })).await;
    assert_eq!(p.body["note"], Value::Null);
    let p = patch(&app, &a, id2, json!({ "note": "x" })).await;
    assert_eq!(p.body["note"], "x");
    let p = patch(&app, &a, id2, json!({ "note": "   " })).await;
    assert_eq!(p.body["note"], Value::Null);
    // Leere Änderung und ungültige Farbe.
    assert_eq!(
        patch(&app, &a, id, json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        patch(&app, &a, id, json!({ "color": "grün" })).await.status,
        StatusCode::BAD_REQUEST
    );

    let d = delete(&app, &format!("/api/highlights/{id}"), Some(&a)).await;
    assert_eq!(d.status, StatusCode::NO_CONTENT);
    assert_eq!(
        delete(&app, &format!("/api/highlights/{id}"), Some(&a))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        list(&app, &a, article).await.body.as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn ungueltige_eingaben_werden_abgelehnt() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let article = seed(&app, "a@example.org", "https://example.org/a").await;
    let long = "x".repeat(2001);
    let cases = [
        json!({ "start": 5, "end": 5, "quote": "x", "color": "#ffeb3b" }),
        json!({ "start": -1, "end": 5, "quote": "x", "color": "#ffeb3b" }),
        json!({ "start": 0, "end": 99_999_999, "quote": "x", "color": "#ffeb3b" }),
        json!({ "start": 0, "end": 3, "quote": "   ", "color": "#ffeb3b" }),
        json!({ "start": 0, "end": 3, "quote": long, "color": "#ffeb3b" }),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "red" }),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "#ffeb3b; x" }),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "#ffeb3b", "note": long }),
        json!({ "start": 0, "end": 3, "quote": "abc" }),
        json!({ "start": "0", "end": 3, "quote": "abc", "color": "#ffeb3b" }),
    ];
    for body in cases {
        let r = create(&app, &a, article, body.clone()).await;
        assert!(r.status.is_client_error(), "{body}: {}", r.status);
    }
    assert_eq!(count(&app.pool, "highlights").await, 0);
}

#[tokio::test]
async fn jeder_sieht_und_aendert_nur_eigene_markierungen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let art_a = seed(&app, "a@example.org", "https://example.org/a").await;
    let art_b = seed(&app, "b@example.org", "https://example.org/b").await;
    let h = create(&app, &a, art_a, new_highlight()).await.body["id"]
        .as_i64()
        .unwrap();

    // B kennt Artikel und Markierung von A nicht.
    assert_eq!(list(&app, &b, art_a).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        create(&app, &b, art_a, new_highlight()).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        patch(&app, &b, h, json!({ "note": "fremd" })).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, &format!("/api/highlights/{h}"), Some(&b))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Auch nicht über den eigenen Artikel.
    assert!(
        list(&app, &b, art_b)
            .await
            .body
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Unverändert bei A.
    let l = list(&app, &a, art_a).await;
    assert_eq!(l.body[0]["note"], "Merken");

    // Ohne Anmeldung nichts.
    let r = get(&app, &format!("/api/articles/{art_a}/highlights"), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn schreibende_anfragen_brauchen_den_csrf_header() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let article = seed(&app, "a@example.org", "https://example.org/a").await;
    let r = call(
        &app,
        Method::POST,
        &format!("/api/articles/{article}/highlights"),
        Some(new_highlight()),
        Some(&a),
        false,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app.pool, "highlights").await, 0);
}

#[tokio::test]
async fn pro_artikel_sind_hoechstens_500_markierungen_erlaubt() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let article = seed(&app, "a@example.org", "https://example.org/a").await;
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500) \
         INSERT INTO highlights (user_id, article_id, start_offset, end_offset, quote, color) \
         SELECT (SELECT id FROM users), ?, i, i + 1, 'x', '#ffeb3b' FROM n",
    )
    .bind(article)
    .execute(&app.pool)
    .await
    .unwrap();
    let r = create(&app, &a, article, new_highlight()).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(count(&app.pool, "highlights").await, 500);
}

#[tokio::test]
async fn beim_loeschen_von_artikel_und_konto_verschwinden_die_markierungen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let art_a = seed(&app, "a@example.org", "https://example.org/a").await;
    let art_b = seed(&app, "b@example.org", "https://example.org/b").await;
    create(&app, &a, art_a, new_highlight()).await;
    create(&app, &b, art_b, new_highlight()).await;
    assert_eq!(count(&app.pool, "highlights").await, 2);

    let d = delete(&app, &format!("/api/articles/{art_a}"), Some(&a)).await;
    assert_eq!(d.status, StatusCode::NO_CONTENT);
    assert_eq!(count(&app.pool, "highlights").await, 1);

    let r = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&b),
        true,
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    assert_eq!(count(&app.pool, "highlights").await, 0);
}

fn exported(reply: &Reply) -> Value {
    reply.body.clone()
}

#[tokio::test]
async fn export_und_import_uebernehmen_markierungen_und_notizen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let art_a = seed(&app, "a@example.org", "https://example.org/a").await;
    seed(&app, "b@example.org", "https://example.org/b").await;
    create(&app, &a, art_a, new_highlight()).await;
    create(
        &app,
        &a,
        art_a,
        json!({ "start": 0, "end": 3, "quote": "Ein", "color": "#123abc" }),
    )
    .await;
    // B hat eine eigene Markierung, die nie in A's Export auftauchen darf.
    let art_b: i64 = sqlx::query_scalar("SELECT id FROM articles WHERE url LIKE '%/b'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    create(
        &app,
        &b,
        art_b,
        json!({ "start": 0, "end": 3, "quote": "Ein", "color": "#000000", "note": "GEHEIM-B" }),
    )
    .await;

    let export = get(&app, "/api/export", Some(&a)).await;
    assert_eq!(export.status, StatusCode::OK);
    let doc = exported(&export);
    let hs = doc["articles"][0]["highlights"].as_array().unwrap();
    assert_eq!(hs.len(), 2);
    assert_eq!(hs[0]["start"], 0, "sortiert nach Position");
    assert_eq!(hs[1]["note"], "Merken");
    assert!(hs[0].get("id").is_none(), "keine Ids in der Datei");
    assert!(!doc.to_string().contains("GEHEIM-B"));

    // Neues Konto C importiert A's Datei.
    let c = register_cookie(&app, "c@example.org").await;
    let r = post(&app, "/api/import", doc.clone(), Some(&c)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["imported"], 1);
    assert_eq!(r.body["highlights"], 2);
    assert_eq!(r.body["highlights_invalid"], 0);
    let art_c: i64 = sqlx::query_scalar(
        "SELECT id FROM articles WHERE user_id = (SELECT id FROM users WHERE email = 'c@example.org')",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let l = list(&app, &c, art_c).await;
    assert_eq!(l.body.as_array().unwrap().len(), 2);
    assert_eq!(l.body[1]["quote"], "Satz");
    assert_eq!(l.body[1]["note"], "Merken");
    assert_eq!(l.body[0]["color"], "#123abc");

    // Der Export von C ist gleich (bis auf den Zeitstempel der Datei).
    let again = exported(&get(&app, "/api/export", Some(&c)).await);
    assert_eq!(again["articles"], doc["articles"]);

    // Zweiter Import: Artikel schon da, Markierungen werden nicht verdoppelt.
    let r = post(&app, "/api/import", doc, Some(&c)).await;
    assert_eq!(r.body["duplicates"], 1);
    assert_eq!(r.body["highlights"], 0);
    assert_eq!(
        list(&app, &c, art_c).await.body.as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn importierte_markierungen_werden_geprueft() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let many: Vec<Value> = (0..600)
        .map(|i| json!({ "start": i, "end": i + 1, "quote": "x", "color": "#ffeb3b" }))
        .collect();
    let mut highlights = vec![
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "#ABCDEF", "note": "ok\u{0007}!",
                "created_at": "2026-01-02T03:04:05Z" }),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "red" }),
        json!({ "start": 5, "end": 2, "quote": "abc", "color": "#ffeb3b" }),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "url(javascript:alert(1))" }),
        json!({ "quote": "ohne Positionen" }),
        json!("kein Objekt"),
        json!({ "start": 0, "end": 3, "quote": "abc", "color": "#ffeb3b",
                "id": 999, "user_id": 999, "created_at": "gestern" }),
    ];
    highlights.extend(many);
    let doc = json!({ "format": "leseliste-export", "version": 2, "articles": [
        { "url": "https://example.org/h", "title": "H", "content": "<p>abc</p>",
          "highlights": highlights }
    ]});
    let r = post(&app, "/api/import", doc, Some(&a)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["imported"], 1);
    assert_eq!(r.body["highlights"], 500, "höchstens 500 je Artikel");
    assert!(r.body["highlights_invalid"].as_i64().unwrap() >= 5);

    let (note, created): (Option<String>, String) =
        sqlx::query_as("SELECT note, created_at FROM highlights WHERE color = '#abcdef'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(note.as_deref(), Some("ok!"));
    assert_eq!(created, "2026-01-02T03:04:05Z");
    // Fremde Ids und ein kaputter Zeitstempel werden ignoriert.
    let owners: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT user_id) FROM highlights")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(owners, 1);
    let bad: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM highlights WHERE created_at = 'gestern'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(bad, 0);
}

#[tokio::test]
async fn alte_exportdateien_ohne_markierungen_lassen_sich_importieren() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let doc = json!({ "format": "leseliste-export", "version": 1, "articles": [
        { "url": "https://example.org/alt", "title": "Alt", "content": "<p>alt</p>" }
    ]});
    let r = post(&app, "/api/import", doc, Some(&a)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["imported"], 1);
    assert_eq!(r.body["highlights"], 0);
}

#[tokio::test]
async fn artikel_antwort_enthaelt_die_eigenen_markierungen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let art_a = seed(&app, "a@example.org", "https://example.org/a").await;
    create(&app, &a, art_a, new_highlight()).await;

    let r = get(&app, &format!("/api/articles/{art_a}"), Some(&a)).await;
    assert_eq!(r.status, StatusCode::OK);
    let hs = r.body["highlights"].as_array().unwrap();
    assert_eq!(hs.len(), 1);
    assert_eq!(hs[0]["quote"], "Satz");
    assert_eq!(hs[0]["note"], "Merken");

    // Ein fremder Benutzer bekommt den Artikel gar nicht erst.
    let r = get(&app, &format!("/api/articles/{art_a}"), Some(&b)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.get("highlights").is_none());
}
