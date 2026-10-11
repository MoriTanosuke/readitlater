//! Integrationstests für Export und Import der eigenen Daten.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::*;
use http_body_util::BodyExt;
use readlater_backend::{
    config::Config,
    ratelimit::{RateLimitConfig, Rule},
};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Legt einen Artikel direkt in der Datenbank an (ohne Abruf) und gibt die ID zurück.
async fn seed(app: &TestApp, user_email: &str, url: &str, title: &str, text: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt, created_at) \
         VALUES ((SELECT id FROM users WHERE email = ?), ?, ?, ?, ?, ?, '2026-03-04T05:06:07Z') \
         RETURNING id",
    )
    .bind(user_email)
    .bind(url)
    .bind(title)
    .bind(format!("<p>{text}</p>"))
    .bind(text)
    .bind(text)
    .fetch_one(&app.pool)
    .await
    .unwrap()
}

async fn tag(app: &TestApp, cookie: &str, id: i64, tags: &[&str]) {
    let reply = call(
        app,
        Method::PATCH,
        &format!("/api/articles/{id}"),
        Some(json!({ "tags": tags, "is_read": true })),
        Some(cookie),
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
}

async fn export(app: &TestApp, cookie: &str) -> Reply {
    get(app, "/api/export", Some(cookie)).await
}

async fn import(app: &TestApp, cookie: &str, file: Value) -> Reply {
    post(app, "/api/import", file, Some(cookie)).await
}

fn file(articles: Value) -> Value {
    json!({ "format": "leseliste-export", "version": 1, "articles": articles })
}

#[tokio::test]
async fn export_enthaelt_nur_eigene_daten_und_ist_ein_normaler_download() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let _b = register_cookie(&app, "b@example.org").await;
    let id = seed(
        &app,
        "a@example.org",
        "https://example.org/a",
        "Artikel A",
        "Text A",
    )
    .await;
    seed(
        &app,
        "b@example.org",
        "https://example.org/b",
        "Geheim B",
        "Text B",
    )
    .await;
    tag(&app, &a, id, &["Rust", "Web"]).await;

    let reply = export(&app, &a).await;
    assert_eq!(reply.status, StatusCode::OK);
    let disposition = reply.headers[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename=\"leseliste-export-"));
    assert!(disposition.ends_with(".json\""));
    assert_eq!(reply.headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");

    let doc = &reply.body;
    assert_eq!(doc["format"], "leseliste-export");
    assert_eq!(doc["version"], 1);
    assert!(doc["exported_at"].as_str().unwrap().ends_with('Z'));
    let articles = doc["articles"].as_array().unwrap();
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0]["url"], "https://example.org/a");
    assert_eq!(articles[0]["title"], "Artikel A");
    assert_eq!(articles[0]["content"], "<p>Text A</p>");
    assert_eq!(articles[0]["content_text"], "Text A");
    assert_eq!(articles[0]["is_read"], true);
    assert_eq!(articles[0]["created_at"], "2026-03-04T05:06:07Z");
    assert_eq!(articles[0]["tags"], json!(["Rust", "Web"]));

    // Weder fremde Daten noch Konto- oder Zugangsdaten.
    let text = doc.to_string();
    for forbidden in [
        "Geheim B",
        "b@example.org",
        "a@example.org",
        "password",
        "token",
        "user_id",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn export_ohne_login_leere_liste_und_fremde_seiten() {
    let app = setup(true).await;
    assert_eq!(
        get(&app, "/api/export", None).await.status,
        StatusCode::UNAUTHORIZED
    );

    let a = register_cookie(&app, "a@example.org").await;
    let empty = export(&app, &a).await;
    assert_eq!(empty.body["articles"], json!([]));

    // Ein Link von einer fremden Seite darf keinen Download auslösen.
    let request = |site: &str| {
        Request::builder()
            .uri("/api/export")
            .header(header::COOKIE, a.as_str())
            .header("sec-fetch-site", site.to_owned())
            .body(Body::empty())
            .unwrap()
    };
    for (site, expected) in [
        ("cross-site", StatusCode::FORBIDDEN),
        ("same-origin", StatusCode::OK),
        ("none", StatusCode::OK),
    ] {
        let response = app.router.clone().oneshot(request(site)).await.unwrap();
        assert_eq!(response.status(), expected, "{site}");
    }
}

#[tokio::test]
async fn export_ueber_mehrere_pakete_ist_vollstaendig_und_geordnet() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    for i in 0..120 {
        seed(
            &app,
            "a@example.org",
            &format!("https://example.org/{i}"),
            &format!("T{i}"),
            "x",
        )
        .await;
    }
    let reply = export(&app, &a).await;
    let urls: Vec<String> = reply.body["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["url"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = (0..120)
        .map(|i| format!("https://example.org/{i}"))
        .collect();
    assert_eq!(urls, expected);
}

#[tokio::test]
async fn export_und_import_ergeben_dieselben_daten_in_einem_neuen_konto() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    let id1 = seed(
        &app,
        "a@example.org",
        "https://example.org/eins",
        "Eins",
        "Käse aus Hessen",
    )
    .await;
    let id2 = seed(
        &app,
        "a@example.org",
        "https://example.org/zwei",
        "Zwei",
        "Brot",
    )
    .await;
    tag(&app, &a, id1, &["Essen", "rust"]).await;
    tag(&app, &a, id2, &[]).await;
    let exported = export(&app, &a).await.body;

    let result = import(&app, &b, exported.clone()).await;
    assert_eq!(result.status, StatusCode::OK, "{:?}", result.body);
    assert_eq!(
        result.body,
        json!({ "imported": 2, "duplicates": 0, "invalid": 0 })
    );

    // Gleiche Artikel, Schlagwörter, Lesestatus und Zeitpunkte; Export von B == Export von A.
    let again = export(&app, &b).await.body;
    assert_eq!(again["articles"], exported["articles"]);

    // Die Volltextsuche findet importierte Artikel (Umlaute gefaltet).
    let found = get(&app, "/api/articles?q=kase", Some(&b)).await;
    assert_eq!(found.body.as_array().unwrap().len(), 1);
    assert_eq!(found.body[0]["title"], "Eins");
    assert_eq!(
        get(&app, "/api/tags", Some(&b))
            .await
            .body
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Die Daten von A sind unverändert, und B hat eigene IDs.
    assert_eq!(
        export(&app, &a).await.body["articles"],
        exported["articles"]
    );
    let own: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM articles WHERE user_id = (SELECT id FROM users WHERE email = ?)",
    )
    .bind("b@example.org")
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(own, 2);
}

#[tokio::test]
async fn zweiter_import_ueberschreibt_nichts() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let id = seed(
        &app,
        "a@example.org",
        "https://example.org/eins",
        "Eins",
        "Text",
    )
    .await;
    let exported = export(&app, &a).await.body;

    // Lokal geändert, danach dieselbe Datei erneut einspielen.
    tag(&app, &a, id, &["neu"]).await;
    let result = import(&app, &a, exported).await;
    assert_eq!(
        result.body,
        json!({ "imported": 0, "duplicates": 1, "invalid": 0 })
    );
    let now = export(&app, &a).await.body;
    assert_eq!(now["articles"][0]["tags"], json!(["neu"]));
    assert_eq!(now["articles"][0]["is_read"], true);
    assert_eq!(count(&app.pool, "articles").await, 1);
}

#[tokio::test]
async fn duplikate_werden_pro_benutzer_und_ohne_fragment_erkannt() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;
    seed(&app, "a@example.org", "https://example.org/x", "X", "Text").await;

    // B hat die Adresse noch nicht: kein Duplikat. Innerhalb der Datei zählt die zweite als Duplikat.
    let entries = json!([
        { "url": "https://example.org/x#abschnitt", "title": "X" },
        { "url": "https://example.org/x", "title": "X nochmal" },
    ]);
    let r = import(&app, &b, file(entries.clone())).await;
    assert_eq!(
        r.body,
        json!({ "imported": 1, "duplicates": 1, "invalid": 0 })
    );
    // A hat sie schon.
    let r = import(&app, &a, file(entries)).await;
    assert_eq!(
        r.body,
        json!({ "imported": 0, "duplicates": 2, "invalid": 0 })
    );
}

#[tokio::test]
async fn importierte_daten_werden_nicht_ungeprueft_uebernommen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let long_title = "T".repeat(1000);
    let many_tags: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
    let entries = json!([
        {
            "url": "https://example.org/ok",
            "title": long_title,
            "content": "<p onclick=\"x()\">Hallo</p><script>alert(1)</script><a href=\"javascript:alert(1)\">b</a>",
            "content_text": "Hallo",
            "created_at": "irgendwann",
            "tags": many_tags,
            "user_id": 999,
            "id": 5,
        },
        { "url": "javascript:alert(1)" },
        { "url": "ftp://example.org/datei" },
        { "url": "https://nutzer:geheim@example.org/" },
        { "url": "x".repeat(3000) },
        { "url": "https://example.org/mit-tags", "tags": ["a,b", "ok", "OK", "", "x".repeat(41), "\u{7}"] },
    ]);
    let r = import(&app, &a, file(entries)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        r.body,
        json!({ "imported": 2, "duplicates": 0, "invalid": 4 })
    );

    let exported = export(&app, &a).await.body;
    let items = exported["articles"].as_array().unwrap();
    let ok = &items[0];
    assert_eq!(ok["title"].as_str().unwrap().chars().count(), 300);
    let content = ok["content"].as_str().unwrap();
    assert!(!content.contains("script") && !content.contains("onclick"));
    assert!(!content.contains("javascript:"));
    assert!(content.contains("Hallo"));
    // Ungültiger Zeitstempel: der aktuelle Zeitpunkt, kein Wert aus der Datei.
    assert_ne!(ok["created_at"], "irgendwann");
    assert!(ok["created_at"].as_str().unwrap().ends_with('Z'));
    // Höchstens 20 Schlagwörter.
    assert_eq!(ok["tags"].as_array().unwrap().len(), 20);
    // Ungültige Schlagwörter fallen weg, Groß-/Kleinschreibung wird zusammengelegt.
    assert_eq!(items[1]["tags"], json!(["ok"]));
    // IDs und Benutzer aus der Datei zählen nicht.
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM articles ORDER BY id")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert!(!ids.contains(&5) || ids.len() == 2);
    let owners: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT user_id FROM articles")
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert_eq!(owners.len(), 1);
    assert_ne!(owners[0], 999);
}

#[tokio::test]
async fn falsche_dateien_werden_abgelehnt_und_aendern_nichts() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let bad = [
        json!({ "format": "pocket", "version": 1, "articles": [] }),
        json!({ "format": "leseliste-export", "version": 2, "articles": [] }),
        json!({ "format": "leseliste-export", "version": 1 }),
        json!([1, 2, 3]),
        json!({ "format": "leseliste-export", "version": 1, "articles": [{ "title": "ohne url" }] }),
        json!({ "format": "leseliste-export", "version": 1, "articles": "nein" }),
    ];
    for doc in bad {
        let r = import(&app, &a, doc.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{doc}");
    }
    // Kein JSON.
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/import")
                .header(header::COOKIE, a.as_str())
                .header("x-requested-with", "test")
                .body(Body::from("das ist kein json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(count(&app.pool, "articles").await, 0);
}

#[tokio::test]
async fn import_braucht_login_csrf_header_und_hat_eigene_groessengrenzen() {
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let doc = file(json!([{ "url": "https://example.org/x" }]));

    assert_eq!(
        post(&app, "/api/import", doc.clone(), None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let no_csrf = call(
        &app,
        Method::POST,
        "/api/import",
        Some(doc),
        Some(&a),
        false,
    )
    .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);

    // Zu viele Artikel.
    let many: Vec<Value> = (0..20_001)
        .map(|i| json!({ "url": format!("https://e.org/{i}") }))
        .collect();
    let r = import(&app, &a, file(Value::Array(many))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Dateien über 64 KiB sind erlaubt (anders als bei den übrigen Routen), über 64 MiB nicht.
    let big = "<p>x</p>".repeat(20_000);
    let r = import(
        &app,
        &a,
        file(json!([{ "url": "https://example.org/gross", "content": big }])),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let huge = "x".repeat(65 * 1024 * 1024);
    let r = import(
        &app,
        &a,
        file(json!([{ "url": "https://example.org/riesig", "content": huge }])),
    )
    .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(count(&app.pool, "articles").await, 1);

    // Ein einzelner Artikel über 4 MiB ist ungültig.
    let r = import(
        &app,
        &a,
        file(json!([{ "url": "https://example.org/zu-gross", "content": "x".repeat(4 * 1024 * 1024 + 1) }])),
    )
    .await;
    assert_eq!(
        r.body,
        json!({ "imported": 0, "duplicates": 0, "invalid": 1 })
    );
}

#[tokio::test]
async fn import_laedt_nichts_aus_dem_netz() {
    // Adressen, die der Abruf-Schutz ablehnen würde, sind als reine Einträge erlaubt,
    // weil nichts abgerufen wird. Die App zeigt sie nur als Verweis an.
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    let r = import(
        &app,
        &a,
        file(json!([{ "url": "http://127.0.0.1:1/intern", "title": "Intern" }])),
    )
    .await;
    assert_eq!(
        r.body,
        json!({ "imported": 1, "duplicates": 0, "invalid": 0 })
    );
}

#[tokio::test]
async fn export_und_import_sind_pro_benutzer_begrenzt() {
    let app = setup_with(Config {
        rate_limits: RateLimitConfig {
            enabled: true,
            general: Rule::new(1000, 1),
            auth: Rule::new(1000, 1),
            login_pair: Rule::new(1000, 1),
            login_email: Rule::new(1000, 1),
            password_user: Rule::new(1000, 1),
            fetch_user: Rule::new(1000, 1),
            export_user: Rule::new(2, 3600),
            import_user: Rule::new(1, 3600),
        },
        ..test_config()
    })
    .await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;

    assert_eq!(export(&app, &a).await.status, StatusCode::OK);
    assert_eq!(export(&app, &a).await.status, StatusCode::OK);
    assert_eq!(export(&app, &a).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(export(&app, &b).await.status, StatusCode::OK);

    assert_eq!(
        import(&app, &a, file(json!([]))).await.status,
        StatusCode::OK
    );
    let blocked = import(&app, &a, file(json!([]))).await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers.contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn export_gibt_den_koerper_stueckweise_aus() {
    // Der Body ist ein Stream, kein vollständig vorab gebauter String.
    let app = setup(true).await;
    let a = register_cookie(&app, "a@example.org").await;
    seed(&app, "a@example.org", "https://example.org/1", "Eins", "x").await;
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/export")
                .header(header::COOKIE, a.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.headers().get(header::CONTENT_LENGTH).is_none());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["articles"].as_array().unwrap().len(), 1);
}
