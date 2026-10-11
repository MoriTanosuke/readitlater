//! Auslieferung des Frontends aus dem Binary (rust-embed) und Sicherheits-Header.
//!
//! Im Release-Build steckt `frontend/public` im Binary, im Debug-Build wird das
//! Verzeichnis bei jeder Anfrage von der Platte gelesen (Änderungen ohne Neubau).
//! Mit dem Feature `rust-embed/debug-embed` lässt sich auch im Debug-Build einbetten.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../frontend/public"]
struct Assets;

/// Bilder aus Artikeln kommen von fremden HTTPS-Servern (ohne Referrer, siehe js/safe-html.js).
const CSP: &str = "default-src 'self'; img-src 'self' https: data:; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

/// Fallback des Routers: liefert eine eingebettete Datei oder 404.
/// Das Routing im Frontend läuft über `#/…`, einen SPA-Fallback braucht es nicht.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    // `/api/...` gehört dem API-Router; unbekannte API-Pfade sollen nie HTML liefern.
    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == etag)
        });

    let mime = if path == "manifest.webmanifest" {
        "application/manifest+json".to_string()
    } else {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_string()
    };

    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else if method == Method::HEAD {
        StatusCode::OK.into_response()
    } else {
        (StatusCode::OK, file.data.into_owned()).into_response()
    };
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    // Immer beim Server nachfragen (sw.js darf nie veraltet im Browser-Cache liegen).
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Sicherheits-Header für alle Antworten (früher in der Caddyfile).
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}
