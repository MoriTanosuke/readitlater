//! Tests für das eingebettete Frontend und die Sicherheits-Header.
mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(
    app: &common::TestApp,
    uri: &str,
    extra: Option<(&str, &str)>,
) -> axum::response::Response {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some((k, v)) = extra {
        b = b.header(k, v);
    }
    app.router
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn index_wird_ausgeliefert_mit_headern() {
    let app = common::setup(true).await;
    let res = get(&app, "/", None).await;
    assert_eq!(res.status(), StatusCode::OK);
    let h = res.headers();
    assert!(
        h[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(h[header::CACHE_CONTROL], "no-cache");
    assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("default-src 'self'")
    );
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("<html"));
}

#[tokio::test]
async fn mime_typen_und_manifest() {
    let app = common::setup(true).await;
    let js = get(&app, "/sw.js", None).await;
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    let css = get(&app, "/style.css", None).await;
    assert!(
        css.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let m = get(&app, "/manifest.webmanifest", None).await;
    assert_eq!(
        m.headers()[header::CONTENT_TYPE],
        "application/manifest+json"
    );
}

#[tokio::test]
async fn webfont_wird_selbst_ausgeliefert() {
    let app = common::setup(true).await;
    for file in [
        "/fonts/inter-latin-wght-normal.woff2",
        "/fonts/inter-latin-ext-wght-normal.woff2",
    ] {
        let res = get(&app, file, None).await;
        assert_eq!(res.status(), StatusCode::OK, "{file}");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "font/woff2", "{file}");
        // Schriften kommen von der eigenen Origin: `default-src 'self'` genügt, es gibt keine
        // eigene `font-src`-Ausnahme für fremde Server.
        let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_string();
        assert!(!csp.contains("font-src"), "{csp}");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..4], b"wOF2", "{file} ist keine WOFF2-Datei");
    }
}

#[tokio::test]
async fn etag_liefert_304() {
    let app = common::setup(true).await;
    let first = get(&app, "/style.css", None).await;
    let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();
    let second = get(&app, "/style.css", Some(("if-none-match", &etag))).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(second.headers()[header::CACHE_CONTROL], "no-cache");
}

#[tokio::test]
async fn unbekannt_ist_404_und_api_bleibt_json() {
    let app = common::setup(true).await;
    assert_eq!(
        get(&app, "/gibt-es-nicht.js", None).await.status(),
        StatusCode::NOT_FOUND
    );
    // Unbekannte API-Pfade liefern nie das Frontend.
    assert_eq!(
        get(&app, "/api/gibt-es-nicht", None).await.status(),
        StatusCode::NOT_FOUND
    );
    let health = get(&app, "/api/health", None).await;
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(health.headers()[header::X_FRAME_OPTIONS], "DENY");
}

#[tokio::test]
async fn nur_get_und_head() {
    let app = common::setup(true).await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}
