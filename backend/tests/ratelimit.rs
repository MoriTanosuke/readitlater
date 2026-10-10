//! Integrationstests für die Ratenbegrenzung. Die Regeln sind klein und fallen praktisch nie
//! zurück (Auffüllzeit eine Stunde), damit die Tests ohne Warten eindeutig sind.

mod common;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Html,
    routing::get as route_get,
};
use common::*;
use http_body_util::BodyExt;
use readlater_backend::{
    config::Config,
    ratelimit::{RateLimitConfig, Rule},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

const WIDE: Rule = Rule::new(1000, 1);
const HOUR: u64 = 3600;

/// Alle Stufen weit offen, einzelne Tests ziehen ihre Stufe enger.
fn wide() -> RateLimitConfig {
    RateLimitConfig {
        enabled: true,
        general: WIDE,
        auth: WIDE,
        login_pair: WIDE,
        login_email: WIDE,
        password_user: WIDE,
        fetch_user: WIDE,
    }
}

async fn app_with(limits: RateLimitConfig) -> TestApp {
    setup_with(Config {
        rate_limits: limits,
        fetch_allow_private: true,
        fetch_timeout_secs: 2,
        ..test_config()
    })
    .await
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// Anfrage wie hinter Caddy: `forwarded` landet im Header `X-Forwarded-For`.
async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    forwarded: &str,
) -> Answer {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-requested-with", "test")
        .header("x-forwarded-for", forwarded);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn login(app: &TestApp, email: &str, password: &str, ip: &str) -> Answer {
    send(
        app,
        Method::POST,
        "/api/login",
        Some(json!({ "email": email, "password": password })),
        None,
        ip,
    )
    .await
}

#[tokio::test]
async fn allgemeines_limit_liefert_429_mit_retry_after_und_nimmt_health_aus() {
    let app = app_with(RateLimitConfig {
        general: Rule::new(3, HOUR),
        ..wide()
    })
    .await;

    for _ in 0..3 {
        let a = send(&app, Method::GET, "/api/me", None, None, "203.0.113.1").await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = send(&app, Method::GET, "/api/me", None, None, "203.0.113.1").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = blocked.headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=HOUR + 1).contains(&retry), "{retry}");
    assert!(
        blocked.body["error"]
            .as_str()
            .unwrap()
            .contains("Zu viele Anfragen")
    );

    // Der Health-Check des Containers wird nie begrenzt.
    for _ in 0..10 {
        let h = send(&app, Method::GET, "/api/health", None, None, "203.0.113.1").await;
        assert_eq!(h.status, StatusCode::OK);
    }
    // Eine andere Adresse hat ihren eigenen Zähler.
    let other = send(&app, Method::GET, "/api/me", None, None, "203.0.113.2").await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gefaelschte_forwarded_for_eintraege_umgehen_das_limit_nicht() {
    let app = app_with(RateLimitConfig {
        general: Rule::new(2, HOUR),
        ..wide()
    })
    .await;
    // Der Proxy hängt die echte Adresse hinten an, der Client schickt wechselnde Vorsätze.
    let statuses: Vec<_> = {
        let mut v = Vec::new();
        for i in 0..4 {
            let forwarded = format!("198.51.100.{i}, 203.0.113.9");
            v.push(
                send(&app, Method::GET, "/api/me", None, None, &forwarded)
                    .await
                    .status,
            );
        }
        v
    };
    assert_eq!(
        statuses,
        [
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );
}

#[tokio::test]
async fn passwort_routen_haben_ein_strengeres_limit_pro_ip() {
    let app = app_with(RateLimitConfig {
        auth: Rule::new(3, HOUR),
        ..wide()
    })
    .await;
    // Verschiedene Adressen für E-Mail: nur die IP-Stufe soll greifen.
    for i in 0..3 {
        let a = login(
            &app,
            &format!("u{i}@example.org"),
            "falsch-falsch-1",
            "203.0.113.5",
        )
        .await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = login(&app, "u9@example.org", "falsch-falsch-1", "203.0.113.5").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);

    // Dieselbe IP wird auch bei Registrierung, Passwort ändern und Konto löschen gebremst.
    for (method, uri) in [
        (Method::POST, "/api/register"),
        (Method::PUT, "/api/account/password"),
        (Method::DELETE, "/api/account"),
    ] {
        let a = send(&app, method, uri, Some(json!({})), None, "203.0.113.5").await;
        assert_eq!(a.status, StatusCode::TOO_MANY_REQUESTS, "{uri}");
    }
    // Andere Routen und andere Adressen sind nicht betroffen.
    let me = send(&app, Method::GET, "/api/me", None, None, "203.0.113.5").await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    let fresh = login(&app, "u0@example.org", "falsch-falsch-1", "203.0.113.6").await;
    assert_eq!(fresh.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_paar_und_konto_gesamt_begrenzen_ohne_das_opfer_sofort_auszusperren() {
    let app = app_with(RateLimitConfig {
        login_pair: Rule::new(2, HOUR),
        login_email: Rule::new(4, HOUR),
        ..wide()
    })
    .await;
    register_cookie(&app, "opfer@example.org").await;

    // Angreifer von einer Adresse: nach 2 Versuchen gesperrt, auch mit richtigem Passwort.
    for _ in 0..2 {
        let a = login(&app, "opfer@example.org", "falsch-falsch-1", "203.0.113.50").await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = login(&app, "opfer@example.org", PASSWORD, "203.0.113.50").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);

    // Das Opfer kann sich von einer anderen Adresse noch anmelden.
    let ok = login(&app, "opfer@example.org", PASSWORD, "203.0.113.51").await;
    assert_eq!(ok.status, StatusCode::OK);

    // Verteilter Angriff: Ist die Obergrenze für das Konto erreicht, gilt sie für alle.
    let last = login(&app, "opfer@example.org", "falsch-falsch-1", "203.0.113.52").await;
    assert_eq!(last.status, StatusCode::UNAUTHORIZED);
    let capped = login(&app, "opfer@example.org", PASSWORD, "203.0.113.53").await;
    assert_eq!(capped.status, StatusCode::TOO_MANY_REQUESTS);

    // Andere Konten sind nicht betroffen.
    let other = login(
        &app,
        "anderer@example.org",
        "falsch-falsch-1",
        "203.0.113.50",
    )
    .await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_schreibweise_der_email_teilt_sich_den_zaehler() {
    let app = app_with(RateLimitConfig {
        login_pair: Rule::new(2, HOUR),
        ..wide()
    })
    .await;
    for email in ["A@Example.org", " a@example.org", "a@EXAMPLE.org"] {
        let a = login(&app, email, "falsch-falsch-1", "203.0.113.60").await;
        if email == "a@EXAMPLE.org" {
            assert_eq!(a.status, StatusCode::TOO_MANY_REQUESTS);
        } else {
            assert_eq!(a.status, StatusCode::UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn passwortaktionen_sind_pro_benutzer_begrenzt() {
    let app = app_with(RateLimitConfig {
        password_user: Rule::new(2, HOUR),
        ..wide()
    })
    .await;
    let cookie = register_cookie(&app, "a@example.org").await;
    for i in 0..2 {
        let a = send(
            &app,
            Method::PUT,
            "/api/account/password",
            Some(json!({ "current_password": "falsch-falsch-1", "new_password": "neues-passwort-1" })),
            Some(&cookie),
            &format!("203.0.113.{i}"),
        )
        .await;
        assert_eq!(a.status, StatusCode::FORBIDDEN);
    }
    // Wechselnde Adressen helfen nicht, der Zähler hängt am Benutzer. Löschen teilt ihn.
    let blocked = send(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": "falsch-falsch-1" })),
        Some(&cookie),
        "203.0.113.99",
    )
    .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(count(&app.pool, "users").await, 1);
}

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
        .route("/a", route_get(|| async { Html(page("A")) }))
        .route("/b", route_get(|| async { Html(page("B")) }))
        .route("/c", route_get(|| async { Html(page("C")) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn abrufe_sind_pro_benutzer_begrenzt_beim_speichern_und_teilen() {
    let app = app_with(RateLimitConfig {
        fetch_user: Rule::new(2, HOUR),
        ..wide()
    })
    .await;
    let site = spawn_site().await;
    let a = register_cookie(&app, "a@example.org").await;
    let b = register_cookie(&app, "b@example.org").await;

    let save = |cookie: String, path: &'static str, ip: &'static str| {
        let url = format!("{site}{path}");
        let app = &app;
        async move {
            send(
                app,
                Method::POST,
                "/api/articles",
                Some(json!({ "url": url })),
                Some(&cookie),
                ip,
            )
            .await
        }
    };

    assert_eq!(
        save(a.clone(), "/a", "203.0.113.1").await.status,
        StatusCode::CREATED
    );
    // Ein Duplikat wird vorher erkannt und verbraucht kein Kontingent.
    assert_eq!(
        save(a.clone(), "/a", "203.0.113.1").await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        save(a.clone(), "/b", "203.0.113.2").await.status,
        StatusCode::CREATED
    );
    // Wechselnde Adressen helfen nicht: das Kontingent hängt am Benutzer.
    let blocked = save(a.clone(), "/c", "203.0.113.3").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers.contains_key(header::RETRY_AFTER));

    // Teilen per Token zählt auf dasselbe Konto.
    let token = send(
        &app,
        Method::POST,
        "/api/share-tokens",
        Some(json!({ "name": "Pixel" })),
        Some(&a),
        "203.0.113.1",
    )
    .await
    .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let shared = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/share")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .header("x-forwarded-for", "203.0.113.4")
                .body(Body::from(format!("{site}/c")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shared.status(), StatusCode::TOO_MANY_REQUESTS);

    // Ein anderer Benutzer hat sein eigenes Kontingent.
    assert_eq!(
        save(b, "/a", "203.0.113.1").await.status,
        StatusCode::CREATED
    );
    assert_eq!(count(&app.pool, "articles").await, 3);
}

#[tokio::test]
async fn ausgeschaltet_wird_nichts_begrenzt() {
    let app = app_with(RateLimitConfig {
        enabled: false,
        general: Rule::new(1, HOUR),
        auth: Rule::new(1, HOUR),
        login_pair: Rule::new(1, HOUR),
        login_email: Rule::new(1, HOUR),
        ..wide()
    })
    .await;
    for _ in 0..5 {
        let a = login(&app, "x@example.org", "falsch-falsch-1", "203.0.113.1").await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn standardwerte_lassen_normale_nutzung_zu() {
    // Mit den echten Standardwerten: Anmelden, ein paar Anfragen, Abmelden.
    let app = setup_with(Config {
        rate_limits: RateLimitConfig::default(),
        ..test_config()
    })
    .await;
    let reg = send(
        &app,
        Method::POST,
        "/api/register",
        Some(json!({ "email": "a@example.org", "password": PASSWORD })),
        None,
        "203.0.113.1",
    )
    .await;
    assert_eq!(reg.status, StatusCode::CREATED);
    let cookie = reg.headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    for _ in 0..30 {
        let a = send(
            &app,
            Method::GET,
            "/api/articles",
            None,
            Some(&cookie),
            "203.0.113.1",
        )
        .await;
        assert_eq!(a.status, StatusCode::OK);
    }
}
