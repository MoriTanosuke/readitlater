//! Integrationstests für das lokale Ablegen und Verkleinern der Artikelbilder.
//!
//! Als „fremde Webseite“ dient ein lokaler Testserver. Der Worker läuft nicht im Hintergrund:
//! Die Tests rufen `images::process_pending` selbst auf, damit nichts auf Zeitgeber wartet.

mod common;

use std::{
    io::Cursor,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Html,
    routing::get as route_get,
};
use common::*;
use http_body_util::BodyExt;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use readlater_backend::{AppState, app, config::Config, db, images};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Testserver
// ---------------------------------------------------------------------------

fn photo(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let (fx, fy) = (f64::from(x), f64::from(y));
        let hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503).rotate_left(13);
        let noise = f64::from((hash >> 7) & 7) - 3.5;
        let channel = |phase: f64| {
            let v = 128.0 + 90.0 * ((fx / 37.0 + phase).sin() * (fy / 53.0 - phase).cos());
            (v + noise).clamp(0.0, 255.0) as u8
        };
        Rgb([channel(0.0), channel(2.0), channel(4.0)])
    })
}

fn encode(img: RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut out), format)
        .unwrap();
    out
}

struct Site {
    addr: SocketAddr,
    /// Aufrufe der Bilder (nicht der Seite).
    image_hits: Arc<AtomicUsize>,
    /// Aufrufe von /bild/flaky.jpg: der erste scheitert mit 503.
    flaky_hits: Arc<AtomicUsize>,
    original_len: usize,
}

impl Site {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

fn article_page(body_images: &str) -> String {
    let satz = "<p>Dies ist ein längerer Absatz mit genug Text, damit die Extraktion den \
                Artikel sicher erkennt und nicht als Navigation oder Rauschen verwirft.</p>";
    format!(
        "<html><head><title>Bilderartikel</title></head><body><article>{}{}{}</article></body></html>",
        satz.repeat(3),
        body_images,
        satz.repeat(3)
    )
}

async fn spawn_site() -> Site {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let image_hits = Arc::new(AtomicUsize::new(0));
    let flaky_hits = Arc::new(AtomicUsize::new(0));
    let big = encode(photo(2400, 1600), ImageFormat::Jpeg);
    let original_len = big.len();

    let hits = image_hits.clone();
    let flaky = flaky_hits.clone();
    let referer_expected = format!("http://{addr}/artikel");
    let router = Router::new()
        .route(
            "/artikel",
            route_get(|| async {
                Html(article_page(
                    "<p><img src=\"/bild/gross.jpg\" alt=\"gross\"></p>\
                     <p><img src=\"/bild/hotlink.jpg\" alt=\"hotlink\"></p>\
                     <p><img src=\"/bild/fehlt.jpg\" alt=\"fehlt\"></p>\
                     <p><img src=\"/bild/kein-bild.svg\" alt=\"svg\"></p>",
                ))
            }),
        )
        .route(
            "/flaky",
            route_get(|| async {
                Html(article_page("<p><img src=\"/bild/flaky.jpg\" alt=\"flaky\"></p>"))
            }),
        )
        .route(
            "/bild/gross.jpg",
            route_get({
                let hits = hits.clone();
                let big = big.clone();
                move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let big = big.clone();
                    async move { ([(header::CONTENT_TYPE, "image/jpeg")], big) }
                }
            }),
        )
        .route(
            "/bild/hotlink.jpg",
            route_get({
                let hits = hits.clone();
                let small = encode(photo(300, 200), ImageFormat::Jpeg);
                move |headers: HeaderMap| {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let ok = headers
                        .get(header::REFERER)
                        .and_then(|v| v.to_str().ok())
                        == Some(referer_expected.as_str());
                    let small = small.clone();
                    async move {
                        if ok {
                            (StatusCode::OK, [(header::CONTENT_TYPE, "image/jpeg")], small)
                        } else {
                            (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/plain")], vec![])
                        }
                    }
                }
            }),
        )
        .route(
            "/bild/fehlt.jpg",
            route_get({
                let hits = hits.clone();
                move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::NOT_FOUND }
                }
            }),
        )
        .route(
            "/bild/kein-bild.svg",
            route_get({
                let hits = hits.clone();
                move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async {
                        (
                            [(header::CONTENT_TYPE, "image/jpeg")], // der Server lügt
                            "<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>",
                        )
                    }
                }
            }),
        )
        .route(
            "/bild/flaky.jpg",
            route_get({
                let ok = encode(photo(300, 200), ImageFormat::Jpeg);
                move || {
                    let first = flaky.fetch_add(1, Ordering::SeqCst) == 0;
                    let ok = ok.clone();
                    async move {
                        if first {
                            (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "text/plain")], vec![])
                        } else {
                            (StatusCode::OK, [(header::CONTENT_TYPE, "image/jpeg")], ok)
                        }
                    }
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Site {
        addr,
        image_hits,
        flaky_hits,
        original_len,
    }
}

// ---------------------------------------------------------------------------
// Anwendung
// ---------------------------------------------------------------------------

struct Env {
    app: TestApp,
    state: AppState,
}

async fn env_with(config: Config) -> Env {
    let pool = db::connect_memory().await.expect("DB");
    let state = AppState::new(pool.clone(), config).expect("AppState");
    Env {
        app: TestApp {
            router: app(state.clone()),
            pool,
        },
        state,
    }
}

async fn env() -> Env {
    env_with(Config {
        fetch_allow_private: true,
        fetch_timeout_secs: 5,
        ..test_config()
    })
    .await
}

async fn save(env: &Env, cookie: &str, url: &str) -> i64 {
    let reply = post(
        &env.app,
        "/api/articles",
        json!({ "url": url }),
        Some(cookie),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply.body);
    reply.body["id"].as_i64().unwrap()
}

async fn article(env: &Env, cookie: &str, id: i64) -> Value {
    let reply = get(&env.app, &format!("/api/articles/{id}"), Some(cookie)).await;
    assert_eq!(reply.status, StatusCode::OK);
    reply.body
}

async fn statuses(env: &Env, article_id: i64) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT substr(url, instr(url, '/bild/')), status FROM article_images \
         WHERE article_id = ? ORDER BY id",
    )
    .bind(article_id)
    .fetch_all(&env.app.pool)
    .await
    .unwrap()
}

fn status_of<'a>(list: &'a [(String, String)], name: &str) -> &'a str {
    list.iter()
        .find(|(url, _)| url == name)
        .map(|(_, s)| s.as_str())
        .unwrap_or_else(|| panic!("{name} fehlt in {list:?}"))
}

/// Antwort ohne JSON-Auswertung (Bilder).
async fn raw_get(env: &Env, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let response = env
        .app
        .router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bilder_werden_im_hintergrund_geholt_verkleinert_und_ausgeliefert() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let id = save(&env, &cookie, &site.url("/artikel")).await;

    // Das Speichern wartet nicht auf die Bilder: sie stehen in der Warteschlange.
    let before = article(&env, &cookie, id).await;
    assert_eq!(before["images"], json!({}));
    assert_eq!(site.image_hits.load(Ordering::SeqCst), 0);
    let rows = statuses(&env, id).await;
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows.iter().all(|(_, s)| s == "pending"));

    assert_eq!(images::process_pending(&env.state).await, 4);

    let rows = statuses(&env, id).await;
    assert_eq!(status_of(&rows, "/bild/gross.jpg"), "done");
    assert_eq!(
        status_of(&rows, "/bild/hotlink.jpg"),
        "done",
        "Referer ist die Artikeladresse"
    );
    assert_eq!(status_of(&rows, "/bild/fehlt.jpg"), "failed");
    assert_eq!(
        status_of(&rows, "/bild/kein-bild.svg"),
        "failed",
        "der Inhalt zählt, nicht der Content-Type"
    );

    // Der Artikel nennt die fertigen Bilder unter ihrer Original-Adresse.
    let after = article(&env, &cookie, id).await;
    let map = after["images"].as_object().unwrap();
    assert_eq!(map.len(), 2, "{map:?}");
    let big = &after["images"][site.url("/bild/gross.jpg")];
    assert_eq!(
        (big["width"].as_i64(), big["height"].as_i64()),
        (Some(1600), Some(1067))
    );
    assert!(after["images"][site.url("/bild/fehlt.jpg")].is_null());
    // Das gespeicherte HTML behält die Original-Adressen.
    assert!(
        after["content"]
            .as_str()
            .unwrap()
            .contains("/bild/gross.jpg")
    );

    // Das Bild ist verkleinert und wird mit eigenen Headern ausgeliefert.
    let src = big["src"].as_str().unwrap();
    assert!(
        src.starts_with(&format!("/api/articles/{id}/images/")),
        "{src}"
    );
    let (status, headers, bytes) = raw_get(&env, src, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "private, max-age=31536000, immutable"
    );
    assert!(
        bytes.len() < site.original_len * 6 / 10,
        "{} gegen {}",
        bytes.len(),
        site.original_len
    );
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1600, 1067));

    // Endgültig gescheiterte Bilder werden nicht erneut geholt.
    let hits = site.image_hits.load(Ordering::SeqCst);
    assert_eq!(images::process_pending(&env.state).await, 0);
    assert_eq!(site.image_hits.load(Ordering::SeqCst), hits);
}

#[tokio::test]
async fn voruebergehende_fehler_werden_spaeter_wiederholt() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let id = save(&env, &cookie, &site.url("/flaky")).await;

    assert_eq!(images::process_pending(&env.state).await, 1);
    let (status, attempts, next_in): (String, i64, i64) = sqlx::query_as(
        "SELECT status, attempts, next_try_at - unixepoch() FROM article_images WHERE article_id = ?",
    )
    .bind(id)
    .fetch_one(&env.app.pool)
    .await
    .unwrap();
    assert_eq!(
        (status.as_str(), attempts),
        ("pending", 1),
        "503 ist vorübergehend"
    );
    assert!(
        (50..=70).contains(&next_in),
        "Wartezeit etwa 60 s, war {next_in}"
    );

    // Noch nicht fällig: nichts passiert.
    assert_eq!(images::process_pending(&env.state).await, 0);
    assert_eq!(site.flaky_hits.load(Ordering::SeqCst), 1);

    // Fällig: zweiter Versuch gelingt.
    sqlx::query("UPDATE article_images SET next_try_at = 0")
        .execute(&env.app.pool)
        .await
        .unwrap();
    assert_eq!(images::process_pending(&env.state).await, 1);
    assert_eq!(statuses(&env, id).await[0].1, "done");
    assert_eq!(
        article(&env, &cookie, id).await["images"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn nach_drei_versuchen_ist_schluss() {
    let env = env().await;
    register(&env.app, "anna@example.com").await;
    // Verbindung verweigert: vorübergehender Fehler, aber nur dreimal.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES ((SELECT id FROM users), 'http://127.0.0.1:1/a', 'T', '<p>x</p>', 'x', '') RETURNING id",
    )
    .fetch_one(&env.app.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO article_images (article_id, url) VALUES (?, 'http://127.0.0.1:1/bild/x.jpg')",
    )
    .bind(id)
    .execute(&env.app.pool)
    .await
    .unwrap();
    for expected in ["pending", "pending", "failed"] {
        images::process_pending(&env.state).await;
        assert_eq!(statuses(&env, id).await[0].1, expected);
        sqlx::query("UPDATE article_images SET next_try_at = 0")
            .execute(&env.app.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn unterbrochene_auftraege_werden_nach_dem_start_wieder_aufgenommen() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let id = save(&env, &cookie, &site.url("/flaky")).await;
    sqlx::query("UPDATE article_images SET status = 'working'")
        .execute(&env.app.pool)
        .await
        .unwrap();
    assert_eq!(
        images::process_pending(&env.state).await,
        0,
        "working wird nicht doppelt bearbeitet"
    );

    images::recover(&env.app.pool).await.unwrap();
    assert_eq!(statuses(&env, id).await[0].1, "pending");
}

#[tokio::test]
async fn bilder_sind_nur_fuer_den_besitzer_erreichbar() {
    let site = spawn_site().await;
    let env = env().await;
    let anna = register_cookie(&env.app, "anna@example.com").await;
    let ben = register_cookie(&env.app, "ben@example.com").await;
    let id = save(&env, &anna, &site.url("/artikel")).await;
    images::process_pending(&env.state).await;
    let src = article(&env, &anna, id).await["images"][site.url("/bild/gross.jpg")]["src"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(raw_get(&env, &src, Some(&anna)).await.0, StatusCode::OK);
    assert_eq!(
        raw_get(&env, &src, Some(&ben)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(raw_get(&env, &src, None).await.0, StatusCode::UNAUTHORIZED);
    // Die Artikel-Id in der Adresse muss zum Bild passen.
    let image_id = src.rsplit('/').next().unwrap();
    let wrong = format!("/api/articles/{}/images/{image_id}", id + 100);
    assert_eq!(
        raw_get(&env, &wrong, Some(&anna)).await.0,
        StatusCode::NOT_FOUND
    );
    // Nicht fertige Bilder gibt es nicht.
    let failed: i64 =
        sqlx::query_scalar("SELECT id FROM article_images WHERE status = 'failed' LIMIT 1")
            .fetch_one(&env.app.pool)
            .await
            .unwrap();
    let failed_src = format!("/api/articles/{id}/images/{failed}");
    assert_eq!(
        raw_get(&env, &failed_src, Some(&anna)).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn artikel_loeschen_entfernt_die_bilder() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let id = save(&env, &cookie, &site.url("/artikel")).await;
    images::process_pending(&env.state).await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM article_images")
            .fetch_one(&env.app.pool)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 4);
    assert_eq!(
        delete(&env.app, &format!("/api/articles/{id}"), Some(&cookie))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(count().await, 0);
}

#[tokio::test]
async fn konto_loeschen_entfernt_die_bilder() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    save(&env, &cookie, &site.url("/artikel")).await;
    images::process_pending(&env.state).await;
    let reply = call(
        &env.app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&cookie),
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{:?}", reply.body);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM article_images")
        .fetch_one(&env.app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn private_ziele_werden_auch_bei_bildern_nie_abgerufen() {
    let site = spawn_site().await;
    // Strenger Modus: der Testserver auf 127.0.0.1 ist tabu.
    let env = env_with(Config {
        fetch_allow_private: false,
        ..test_config()
    })
    .await;
    register(&env.app, "anna@example.com").await;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES ((SELECT id FROM users), 'https://example.org/a', 'T', '<p>x</p>', 'x', '') RETURNING id",
    )
    .fetch_one(&env.app.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO article_images (article_id, url) VALUES (?, ?)")
        .bind(id)
        .bind(site.url("/bild/gross.jpg"))
        .execute(&env.app.pool)
        .await
        .unwrap();
    assert_eq!(images::process_pending(&env.state).await, 1);
    assert_eq!(statuses(&env, id).await[0].1, "failed");
    assert_eq!(site.image_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pro_artikel_werden_hoechstens_30_bilder_eingereiht() {
    let env = env().await;
    register(&env.app, "anna@example.com").await;
    let imgs: String = (0..40)
        .map(|i| format!("<img src=\"https://example.org/bild/{i}.jpg\">"))
        .collect();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES ((SELECT id FROM users), 'https://example.org/a', 'T', ?, 'x', '') RETURNING id",
    )
    .bind(format!("<p>{imgs}</p>"))
    .fetch_one(&env.app.pool)
    .await
    .unwrap();
    let added = images::enqueue(
        &env.state,
        id,
        &format!("<p>{imgs}</p>"),
        &"https://example.org/a".parse().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(added, 30);
}

#[tokio::test]
async fn abgeschaltet_wird_nichts_eingereiht() {
    let site = spawn_site().await;
    let env = env_with(Config {
        fetch_allow_private: true,
        image_store_enabled: false,
        ..test_config()
    })
    .await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let id = save(&env, &cookie, &site.url("/artikel")).await;
    assert!(statuses(&env, id).await.is_empty());
    let reply = post(
        &env.app,
        &format!("/api/articles/{id}/refresh-images"),
        json!({}),
        Some(&cookie),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn nachladen_reiht_aeltere_artikel_ein_und_wiederholt_gescheiterte() {
    let site = spawn_site().await;
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let ben = register_cookie(&env.app, "ben@example.com").await;
    let id = save(&env, &cookie, &site.url("/artikel")).await;
    images::process_pending(&env.state).await;
    let hits = site.image_hits.load(Ordering::SeqCst);

    // Ein Artikel von früher (ohne Bilderzeilen) und ein Bild, das inzwischen erreichbar wäre.
    sqlx::query("DELETE FROM article_images WHERE url LIKE '%gross%'")
        .execute(&env.app.pool)
        .await
        .unwrap();
    let url = format!("/api/articles/{id}/refresh-images");
    assert_eq!(
        post(&env.app, &url, json!({}), Some(&ben)).await.status,
        StatusCode::NOT_FOUND
    );
    let reply = post(&env.app, &url, json!({}), Some(&cookie)).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    assert_eq!(
        reply.body["queued"], 3,
        "gross.jpg neu, fehlt.jpg und das SVG nochmals"
    );

    images::process_pending(&env.state).await;
    let rows = statuses(&env, id).await;
    assert_eq!(status_of(&rows, "/bild/gross.jpg"), "done");
    assert_eq!(status_of(&rows, "/bild/hotlink.jpg"), "done");
    assert_eq!(status_of(&rows, "/bild/fehlt.jpg"), "failed");
    assert!(site.image_hits.load(Ordering::SeqCst) > hits);
    // Fertige Bilder bleiben unverändert (Cache-Header „immutable“ stimmt).
    assert_eq!(
        article(&env, &cookie, id).await["images"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn import_holt_keine_bilder() {
    // Importierte Artikel erzeugen keine Warteschlange: Dateiinhalte werden nie abgerufen.
    let env = env().await;
    let cookie = register_cookie(&env.app, "anna@example.com").await;
    let file = json!({
        "format": "leseliste-export", "version": 2, "exported_at": "2026-01-01T00:00:00Z",
        "articles": [{
            "url": "https://example.org/a", "title": "T", "excerpt": "", "is_read": false,
            "created_at": "2026-01-01T00:00:00Z", "tags": [],
            "content": "<p>Text genug, um gültig zu sein. <img src=\"https://example.org/x.jpg\"></p>",
            "highlights": []
        }]
    });
    let reply = post(&env.app, "/api/import", file, Some(&cookie)).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM article_images")
        .fetch_one(&env.app.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}
