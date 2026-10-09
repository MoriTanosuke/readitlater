//! Integrationstests für Artikel speichern, lesen, auflisten, löschen und den SSRF-Schutz.
//!
//! Als „fremde Webseite“ dient ein lokaler Testserver. Weil der SSRF-Schutz Zugriffe auf
//! 127.0.0.1 blockiert, läuft der Großteil der Tests mit `fetch_allow_private = true`.
//! Die Tests im strengen Modus belegen dagegen, dass genau diese Zugriffe abgelehnt werden.

mod common;

use std::{
    io::Write,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, Redirect},
    routing::get as route_get,
};
use common::*;
use flate2::{Compression, write::GzEncoder};
use readlater_backend::config::Config;
use serde_json::json;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Testserver
// ---------------------------------------------------------------------------

fn article_page(title: &str) -> String {
    let absatz = "<p>Dies ist ein ausführlicher Absatz mit genug Text, damit die Extraktion ihn \
                  sicher als Hauptinhalt der Seite erkennt und nicht als Beiwerk verwirft.</p>";
    format!(
        "<!doctype html><html><head><title>{title}</title></head><body>\
         <nav><a href=\"/\">Start</a></nav>\
         <article>{}\
         <script>alert('xss')</script>\
         <p onclick=\"steal()\">Link: <a href=\"javascript:alert(1)\">böse</a> und \
         <a href=\"https://example.org/ziel\">gut</a>.</p>\
         <img src=\"https://example.org/bild.png\" onerror=\"steal()\">\
         </article><footer>Impressum</footer></body></html>",
        absatz.repeat(6)
    )
}

fn latin1_page() -> Vec<u8> {
    let mut body = b"<html><head><title>K\xe4se aus Hessen</title></head><body><article>".to_vec();
    for _ in 0..6 {
        body.extend_from_slice(
            b"<p>Ein l\xe4ngerer Absatz mit Umlauten und genug Text f\xfcr die Extraktion des \
              Hauptinhalts der Seite.</p>",
        );
    }
    body.extend_from_slice(b"</article></body></html>");
    body
}

fn gzip_bomb() -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&b"x".repeat(500_000)).unwrap();
    encoder.finish().unwrap()
}

/// Startet den Testserver und liefert Adresse sowie einen Zähler für eingegangene Anfragen.
async fn spawn_site() -> (SocketAddr, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/artikel",
            route_get(|| async { Html(article_page("Erster Artikel")) }),
        )
        .route(
            "/zweiter",
            route_get(|| async { Html(article_page("Zweiter Artikel")) }),
        )
        .route(
            "/weiter",
            route_get(|| async { Redirect::temporary("/artikel") }),
        )
        .route(
            "/text",
            route_get(|| async { ([(header::CONTENT_TYPE, "text/plain")], "nur Text") }),
        )
        .route("/gross", route_get(|| async { Html("x".repeat(200_000)) }))
        .route(
            "/bombe",
            route_get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/html"),
                        (header::CONTENT_ENCODING, "gzip"),
                    ],
                    gzip_bomb(),
                )
            }),
        )
        .route(
            "/langsam",
            route_get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                Html(article_page("Langsam"))
            }),
        )
        .route(
            "/leer",
            route_get(|| async { Html("<html><body><p>Kurz</p></body></html>") }),
        )
        .route(
            "/latin1",
            route_get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=iso-8859-1")],
                    latin1_page(),
                )
            }),
        )
        .layer(middleware::from_fn_with_state(
            hits.clone(),
            |State(hits): State<Arc<AtomicUsize>>, request: Request, next: Next| async move {
                hits.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            },
        ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, hits)
}

/// App, die Abrufe auf den lokalen Testserver erlaubt (kleine Limits für schnelle Tests).
async fn app_with_local_fetch() -> TestApp {
    setup_with(Config {
        fetch_allow_private: true,
        fetch_max_bytes: 100_000,
        fetch_timeout_secs: 1,
        ..test_config()
    })
    .await
}

async fn add(app: &TestApp, cookie: &str, url: &str) -> Reply {
    post(app, "/api/articles", json!({ "url": url }), Some(cookie)).await
}

fn site_url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

// ---------------------------------------------------------------------------
// Speichern und Lesen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn speichern_extrahiert_und_bereinigt_den_inhalt() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    let reply = add(&app, &cookie, &site_url(addr, "/artikel")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.error());
    assert_eq!(reply.body["title"], "Erster Artikel");
    assert_eq!(reply.body["url"], site_url(addr, "/artikel"));
    assert_eq!(reply.body["is_read"], false);
    assert!(!reply.body["excerpt"].as_str().unwrap().is_empty());
    assert!(reply.body.get("content").is_none(), "Liste ohne Volltext");
    let id = reply.body["id"].as_i64().unwrap();

    let full = get(&app, &format!("/api/articles/{id}"), Some(&cookie)).await;
    assert_eq!(full.status, StatusCode::OK);
    let content = full.body["content"].as_str().unwrap().to_lowercase();
    assert!(content.contains("ausführlicher absatz"), "{content}");
    for verboten in ["<script", "onclick", "onerror", "javascript:", "steal"] {
        assert!(!content.contains(verboten), "{verboten} in {content}");
    }
    assert!(content.contains("https://example.org/ziel"));
    assert!(content.contains("noopener noreferrer nofollow"));

    let text: String = sqlx::query_scalar("SELECT content_text FROM articles WHERE id = ?")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(text.contains("ausführlicher Absatz"));
    assert!(!text.contains("<p>"));
}

#[tokio::test]
async fn weiterleitungen_werden_gefolgt() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    let reply = add(&app, &cookie, &site_url(addr, "/weiter")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.error());
    assert_eq!(reply.body["title"], "Erster Artikel");
    assert_eq!(reply.body["url"], site_url(addr, "/weiter"));
}

#[tokio::test]
async fn zeichensatz_der_seite_wird_beachtet() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    let reply = add(&app, &cookie, &site_url(addr, "/latin1")).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.error());
    assert_eq!(reply.body["title"], "Käse aus Hessen");
}

#[tokio::test]
async fn doppelte_adressen_werden_abgelehnt() {
    let (addr, hits) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;

    let erste = add(&app, &anna, &site_url(addr, "/artikel")).await;
    assert_eq!(erste.status, StatusCode::CREATED, "{}", erste.error());
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Gleiche Adresse, auch mit Fragment: Konflikt, ohne die Seite erneut zu laden.
    for url in [
        site_url(addr, "/artikel"),
        format!("{}#abschnitt", site_url(addr, "/artikel")),
    ] {
        let zweite = add(&app, &anna, &url).await;
        assert_eq!(zweite.status, StatusCode::CONFLICT, "{url}");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(count(&app.pool, "articles").await, 1);

    // Ein anderer Benutzer darf dieselbe Adresse speichern.
    let fremd = add(&app, &bert, &site_url(addr, "/artikel")).await;
    assert_eq!(fremd.status, StatusCode::CREATED);
    assert_eq!(count(&app.pool, "articles").await, 2);
}

#[tokio::test]
async fn liste_zeigt_nur_eigene_artikel_neueste_zuerst() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;

    add(&app, &anna, &site_url(addr, "/artikel")).await;
    add(&app, &anna, &site_url(addr, "/zweiter")).await;
    add(&app, &bert, &site_url(addr, "/artikel")).await;

    let liste = get(&app, "/api/articles", Some(&anna)).await;
    assert_eq!(liste.status, StatusCode::OK);
    let titel: Vec<&str> = liste
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(titel, ["Zweiter Artikel", "Erster Artikel"]);

    let begrenzt = get(&app, "/api/articles?limit=1&offset=1", Some(&anna)).await;
    let begrenzt = begrenzt.body.as_array().unwrap().clone();
    assert_eq!(begrenzt.len(), 1);
    assert_eq!(begrenzt[0]["title"], "Erster Artikel");

    let bert_liste = get(&app, "/api/articles", Some(&bert)).await;
    assert_eq!(bert_liste.body.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Zugriffsschutz und Löschen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fremde_artikel_sind_weder_lesbar_noch_loeschbar() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let bert = register_cookie(&app, "bert@example.com").await;

    let id = add(&app, &anna, &site_url(addr, "/artikel")).await.body["id"]
        .as_i64()
        .unwrap();
    let pfad = format!("/api/articles/{id}");

    assert_eq!(
        get(&app, &pfad, Some(&bert)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, &pfad, Some(&bert)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&app, &pfad, Some(&anna)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn ohne_login_und_ohne_csrf_header_kein_zugriff() {
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    assert_eq!(
        get(&app, "/api/articles", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/api/articles/1", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        delete(&app, "/api/articles/1", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let anonym = post(
        &app,
        "/api/articles",
        json!({ "url": "https://example.com" }),
        None,
    )
    .await;
    assert_eq!(anonym.status, StatusCode::UNAUTHORIZED);

    let ohne_header = call(
        &app,
        Method::POST,
        "/api/articles",
        Some(json!({ "url": "https://example.com" })),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(ohne_header.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn loeschen_entfernt_artikel_und_schlagwort_zuordnungen() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let anna = register_cookie(&app, "anna@example.com").await;
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let id = add(&app, &anna, &site_url(addr, "/artikel")).await.body["id"]
        .as_i64()
        .unwrap();
    let tag_id = sqlx::query("INSERT INTO tags (user_id, name) VALUES (?, 'rust')")
        .bind(user_id)
        .execute(&app.pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query("INSERT INTO article_tags (article_id, tag_id) VALUES (?, ?)")
        .bind(id)
        .bind(tag_id)
        .execute(&app.pool)
        .await
        .unwrap();

    let pfad = format!("/api/articles/{id}");
    assert_eq!(
        delete(&app, &pfad, Some(&anna)).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(&app, &pfad, Some(&anna)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        delete(&app, &pfad, Some(&anna)).await.status,
        StatusCode::NOT_FOUND
    );

    assert_eq!(count(&app.pool, "articles").await, 0);
    assert_eq!(count(&app.pool, "article_tags").await, 0);
    assert_eq!(
        count(&app.pool, "tags").await,
        1,
        "das Schlagwort selbst bleibt"
    );
}

// ---------------------------------------------------------------------------
// Fehlerfälle beim Laden
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unbrauchbare_antworten_werden_abgelehnt_und_nicht_gespeichert() {
    let (addr, _) = spawn_site().await;
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    let faelle = [
        ("/text", StatusCode::UNPROCESSABLE_ENTITY),  // kein HTML
        ("/gross", StatusCode::UNPROCESSABLE_ENTITY), // über dem Größenlimit (Content-Length)
        ("/bombe", StatusCode::UNPROCESSABLE_ENTITY), // entpackt über dem Limit
        ("/leer", StatusCode::UNPROCESSABLE_ENTITY),  // kein Artikeltext
        ("/gibt-es-nicht", StatusCode::BAD_GATEWAY),  // 404 der fremden Seite
        ("/langsam", StatusCode::BAD_GATEWAY),        // Timeout
    ];
    for (pfad, erwartet) in faelle {
        let reply = add(&app, &cookie, &site_url(addr, pfad)).await;
        assert_eq!(reply.status, erwartet, "{pfad}: {}", reply.error());
        assert!(!reply.error().is_empty(), "{pfad}");
    }
    assert_eq!(count(&app.pool, "articles").await, 0);
}

#[tokio::test]
async fn nicht_erreichbare_server_liefern_bad_gateway() {
    // Port, auf dem niemand lauscht.
    let frei = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let app = app_with_local_fetch().await;
    let cookie = register_cookie(&app, "anna@example.com").await;
    let reply = add(&app, &cookie, &site_url(frei, "/artikel")).await;
    assert_eq!(reply.status, StatusCode::BAD_GATEWAY, "{}", reply.error());
}

// ---------------------------------------------------------------------------
// SSRF-Schutz (Standardkonfiguration)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn private_und_lokale_ziele_werden_nie_abgerufen() {
    let (addr, hits) = spawn_site().await;
    // Strenger Modus: Standardwerte, `fetch_allow_private` ist aus.
    let app = setup_with(test_config()).await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    let blockiert = [
        site_url(addr, "/artikel"),                          // 127.0.0.1 direkt
        format!("http://localhost:{}/artikel", addr.port()), // Hostname, der lokal auflöst
        format!("http://[::1]:{}/artikel", addr.port()),
        format!("http://2130706433:{}/artikel", addr.port()), // 127.0.0.1 als Dezimalzahl
        format!("http://0x7f.1:{}/artikel", addr.port()),
        format!("http://[::ffff:127.0.0.1]:{}/artikel", addr.port()),
        "http://169.254.169.254/latest/meta-data/".to_string(), // Cloud-Metadaten
        "http://10.0.0.5/".to_string(),
        "http://192.168.178.1/".to_string(),
        "http://172.16.0.1/".to_string(),
        "http://100.64.0.1/".to_string(),
        "http://0.0.0.0/".to_string(),
    ];
    for url in blockiert {
        let reply = add(&app, &cookie, &url).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{url}: {}",
            reply.error()
        );
        assert_eq!(reply.error(), "Diese Adresse ist nicht erlaubt", "{url}");
    }

    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "der Testserver darf nie erreicht werden"
    );
    assert_eq!(count(&app.pool, "articles").await, 0);
}

#[tokio::test]
async fn ungueltige_adressen_werden_abgelehnt() {
    let app = setup_with(test_config()).await;
    let cookie = register_cookie(&app, "anna@example.com").await;

    for url in [
        "",
        "   ",
        "ftp://example.com/datei",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "http://user:passwort@example.com/",
        "http://",
        "https://exa mple.com/",
    ] {
        let reply = add(&app, &cookie, url).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{url:?}: {}",
            reply.error()
        );
    }
    let zu_lang = format!("https://example.com/{}", "a".repeat(3000));
    assert_eq!(
        add(&app, &cookie, &zu_lang).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(count(&app.pool, "articles").await, 0);
}
