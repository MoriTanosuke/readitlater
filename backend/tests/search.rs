//! Integrationstests für Schlagwörter, Gelesen-Status und die Volltextsuche.
//!
//! Die Artikel werden direkt in die Datenbank geschrieben, damit kein Webserver nötig ist.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

async fn user_id(app: &TestApp, cookie: &str) -> i64 {
    get(app, "/api/me", Some(cookie)).await.body["id"]
        .as_i64()
        .unwrap()
}

async fn insert(app: &TestApp, user: i64, url: &str, title: &str, text: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user)
    .bind(url)
    .bind(title)
    .bind(format!("<p>{text}</p>"))
    .bind(text)
    .bind(text)
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

fn ids(reply: &Reply) -> Vec<i64> {
    reply
        .body
        .as_array()
        .expect("Liste")
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn suche_findet_woerter_praefixe_und_umlaute() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let uid = user_id(&app, &cookie).await;
    let rust = insert(
        &app,
        uid,
        "https://a.example/1",
        "Rust im Alltag",
        "Speichersicherheit ohne Garbage Collector",
    )
    .await;
    let kaffee = insert(
        &app,
        uid,
        "https://a.example/2",
        "Kaffee",
        "Die Müllerin röstet Bohnen",
    )
    .await;

    let q = |text: &str| format!("/api/articles?q={}", text.replace(' ', "%20"));

    // Wort im Text, im Titel, als Präfix und mit anderer Schreibweise.
    assert_eq!(ids(&get(&app, &q("garbage"), Some(&cookie)).await), [rust]);
    assert_eq!(ids(&get(&app, &q("rust"), Some(&cookie)).await), [rust]);
    assert_eq!(ids(&get(&app, &q("speicher"), Some(&cookie)).await), [rust]);
    assert_eq!(
        ids(&get(&app, &q("mullerin"), Some(&cookie)).await),
        [kaffee]
    );
    assert_eq!(
        ids(&get(&app, &q("müllerin"), Some(&cookie)).await),
        [kaffee]
    );

    // Mehrere Wörter: alle müssen vorkommen.
    assert_eq!(
        ids(&get(&app, &q("garbage collector"), Some(&cookie)).await),
        [rust]
    );
    assert!(ids(&get(&app, &q("garbage bohnen"), Some(&cookie)).await).is_empty());

    // Auszug mit Markierung, nur bei einer Suche.
    let treffer = get(&app, &q("collector"), Some(&cookie)).await;
    let snippet = treffer.body[0]["snippet"].as_str().unwrap();
    assert!(snippet.contains("\u{1}Collector\u{2}"), "{snippet:?}");
    let liste = get(&app, "/api/articles", Some(&cookie)).await;
    assert!(liste.body[0].get("snippet").is_none());
}

#[tokio::test]
async fn suche_ignoriert_operatoren_und_sonderzeichen() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let uid = user_id(&app, &cookie).await;
    insert(
        &app,
        uid,
        "https://a.example/1",
        "Titel",
        "Ein ganz normaler Text",
    )
    .await;

    // Diese Eingaben sind in FTS5 Syntax; sie dürfen weder Fehler noch Treffer erzeugen.
    for q in [
        "%22",
        "%22%22",
        "-",
        "*",
        "(",
        "ein%20OR%20x",
        "NEAR(a%20b)",
        "title%3Atitel%20foo",
        "%5E",
        "a%22b",
    ] {
        let reply = get(&app, &format!("/api/articles?q={q}"), Some(&cookie)).await;
        assert_eq!(reply.status, StatusCode::OK, "q={q}: {:?}", reply.body);
    }
    // „OR“ ist ein gewöhnliches Wort: nur Artikel mit „ein“ UND „or“ würden passen.
    assert!(ids(&get(&app, "/api/articles?q=ein%20OR%20x", Some(&cookie)).await).is_empty());

    let zu_lang = get(
        &app,
        &format!("/api/articles?q={}", "a".repeat(201)),
        Some(&cookie),
    )
    .await;
    assert_eq!(zu_lang.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn suche_zeigt_nur_eigene_artikel_und_folgt_aenderungen() {
    let app = setup(true).await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;
    let (a, b) = (user_id(&app, &anna).await, user_id(&app, &bert).await);
    let ann_art = insert(&app, a, "https://a.example/1", "Geheimnis", "Pfefferminz").await;
    insert(&app, b, "https://a.example/1", "Geheimnis", "Pfefferminz").await;

    assert_eq!(
        ids(&get(&app, "/api/articles?q=pfefferminz", Some(&anna)).await),
        [ann_art]
    );

    // Update-Trigger: neuer Text ist auffindbar, alter nicht mehr.
    sqlx::query("UPDATE articles SET content_text = 'Kamille' WHERE id = ?")
        .bind(ann_art)
        .execute(&app.pool)
        .await
        .unwrap();
    assert!(ids(&get(&app, "/api/articles?q=pfefferminz", Some(&anna)).await).is_empty());
    assert_eq!(
        ids(&get(&app, "/api/articles?q=kamille", Some(&anna)).await),
        [ann_art]
    );

    // Delete-Trigger über die API.
    assert_eq!(
        delete(&app, &format!("/api/articles/{ann_art}"), Some(&anna))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(ids(&get(&app, "/api/articles?q=kamille", Some(&anna)).await).is_empty());
    assert_eq!(
        ids(&get(&app, "/api/articles?q=pfefferminz", Some(&bert)).await).len(),
        1
    );

    // Konto löschen (CASCADE) räumt auch den Index auf.
    let weg = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&bert),
        true,
    )
    .await;
    assert_eq!(weg.status, StatusCode::NO_CONTENT);
    let rest: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM articles_fts WHERE articles_fts MATCH 'pfefferminz'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(rest, 0);
}

#[tokio::test]
async fn gelesen_status_aendern_und_filtern() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let uid = user_id(&app, &cookie).await;
    let eins = insert(&app, uid, "https://a.example/1", "Eins", "alpha").await;
    let zwei = insert(&app, uid, "https://a.example/2", "Zwei", "alpha").await;

    let reply = patch(&app, &cookie, eins, json!({ "is_read": true })).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["is_read"], true);

    assert_eq!(
        ids(&get(&app, "/api/articles?read=true", Some(&cookie)).await),
        [eins]
    );
    assert_eq!(
        ids(&get(&app, "/api/articles?read=false", Some(&cookie)).await),
        [zwei]
    );
    assert_eq!(
        ids(&get(&app, "/api/articles?read=false&q=alpha", Some(&cookie)).await),
        [zwei]
    );
    assert_eq!(
        ids(&get(&app, "/api/articles", Some(&cookie)).await),
        [zwei, eins]
    );

    // Zurück auf ungelesen; ein leerer PATCH ändert nichts.
    assert_eq!(
        patch(&app, &cookie, eins, json!({ "is_read": false }))
            .await
            .body["is_read"],
        false
    );
    assert_eq!(
        patch(&app, &cookie, eins, json!({})).await.status,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, &format!("/api/articles/{eins}"), Some(&cookie))
            .await
            .body["is_read"],
        false
    );
}

#[tokio::test]
async fn schlagwoerter_setzen_filtern_und_aufraeumen() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let uid = user_id(&app, &cookie).await;
    let eins = insert(&app, uid, "https://a.example/1", "Eins", "alpha").await;
    let zwei = insert(&app, uid, "https://a.example/2", "Zwei", "alpha").await;

    let reply = patch(
        &app,
        &cookie,
        eins,
        json!({ "tags": [" Rust ", "web  dev", "rust", ""] }),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["tags"], json!(["Rust", "web dev"]));
    // Dasselbe Schlagwort in anderer Schreibweise ist dasselbe Schlagwort.
    let reply = patch(&app, &cookie, zwei, json!({ "tags": ["RUST"] })).await;
    assert_eq!(reply.body["tags"], json!(["Rust"]));
    assert_eq!(count(&app.pool, "tags").await, 2);

    // Anzeige in Liste und Einzelansicht, Filter (ohne Beachtung der Schreibweise).
    let liste = get(&app, "/api/articles", Some(&cookie)).await;
    assert_eq!(liste.body[1]["tags"], json!(["Rust", "web dev"]));
    assert_eq!(
        get(&app, &format!("/api/articles/{eins}"), Some(&cookie))
            .await
            .body["tags"],
        json!(["Rust", "web dev"])
    );
    assert_eq!(
        ids(&get(&app, "/api/articles?tag=rust", Some(&cookie)).await),
        [zwei, eins]
    );
    assert_eq!(
        ids(&get(&app, "/api/articles?tag=web%20dev", Some(&cookie)).await),
        [eins]
    );
    assert_eq!(
        ids(&get(&app, "/api/articles?tag=web%20dev&q=alpha", Some(&cookie)).await),
        [eins]
    );
    assert!(ids(&get(&app, "/api/articles?tag=unbekannt", Some(&cookie)).await).is_empty());

    // Übersicht mit Anzahl.
    let tags = get(&app, "/api/tags", Some(&cookie)).await;
    assert_eq!(
        tags.body,
        json!([{ "name": "Rust", "count": 2 }, { "name": "web dev", "count": 1 }])
    );

    // Ersetzen entfernt nicht mehr genutzte Schlagwörter, andere Felder bleiben unberührt.
    let reply = patch(&app, &cookie, eins, json!({ "tags": [] })).await;
    assert_eq!(reply.body["tags"], json!([]));
    assert_eq!(count(&app.pool, "tags").await, 1);
    assert_eq!(count(&app.pool, "article_tags").await, 1);
}

#[tokio::test]
async fn schlagwoerter_sind_pro_benutzer_getrennt() {
    let app = setup(true).await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;
    let (a, b) = (user_id(&app, &anna).await, user_id(&app, &bert).await);
    let ann_art = insert(&app, a, "https://a.example/1", "Eins", "alpha").await;
    let bert_art = insert(&app, b, "https://a.example/1", "Eins", "alpha").await;

    patch(&app, &anna, ann_art, json!({ "tags": ["gemeinsam"] })).await;
    patch(
        &app,
        &bert,
        bert_art,
        json!({ "tags": ["gemeinsam", "nur-bert"] }),
    )
    .await;
    assert_eq!(count(&app.pool, "tags").await, 3);

    // Fremde Artikel lassen sich weder ändern noch sehen.
    let fremd = patch(
        &app,
        &anna,
        bert_art,
        json!({ "is_read": true, "tags": ["x"] }),
    )
    .await;
    assert_eq!(fremd.status, StatusCode::NOT_FOUND);
    let unveraendert = get(&app, &format!("/api/articles/{bert_art}"), Some(&bert)).await;
    assert_eq!(unveraendert.body["is_read"], false);
    assert_eq!(unveraendert.body["tags"], json!(["gemeinsam", "nur-bert"]));

    let tags = get(&app, "/api/tags", Some(&anna)).await;
    assert_eq!(tags.body, json!([{ "name": "gemeinsam", "count": 1 }]));
    assert!(ids(&get(&app, "/api/articles?tag=nur-bert", Some(&anna)).await).is_empty());
    assert_eq!(
        patch(&app, &anna, 99999, json!({})).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn ungueltige_aenderungen_werden_abgelehnt_ohne_etwas_zu_aendern() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let uid = user_id(&app, &cookie).await;
    let id = insert(&app, uid, "https://a.example/1", "Eins", "alpha").await;
    patch(&app, &cookie, id, json!({ "tags": ["alt"] })).await;

    let viele: Vec<String> = (0..21).map(|i| format!("t{i}")).collect();
    for body in [
        json!({ "tags": ["a,b"] }),
        json!({ "tags": ["x".repeat(41)] }),
        json!({ "tags": viele }),
        json!({ "tags": "kein-array" }),
        json!({ "is_read": "ja" }),
    ] {
        let reply = patch(&app, &cookie, id, body.clone()).await;
        assert!(reply.status.is_client_error(), "{body}: {}", reply.status);
    }
    let article = get(&app, &format!("/api/articles/{id}"), Some(&cookie)).await;
    assert_eq!(article.body["tags"], json!(["alt"]));

    // Ohne Login und ohne CSRF-Header geht nichts.
    let ohne_login = call(
        &app,
        Method::PATCH,
        &format!("/api/articles/{id}"),
        Some(json!({})),
        None,
        true,
    )
    .await;
    assert_eq!(ohne_login.status, StatusCode::UNAUTHORIZED);
    let ohne_csrf = call(
        &app,
        Method::PATCH,
        &format!("/api/articles/{id}"),
        Some(json!({})),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(ohne_csrf.status, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&app, "/api/tags", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}
