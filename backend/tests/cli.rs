//! Tests für den Betreiberbefehl `reset-credentials` (Logik in `readlater_backend::cli`).

mod common;

use axum::http::StatusCode;
use common::{authenticator::SoftAuthenticator, *};
use readlater_backend::{
    cli::{self, ResetError, ResetOptions, ResetReport},
    config::Config,
};
use serde_json::json;

const NEW_PASSWORD: &str = "ein-ganz-neues-passwort";

fn options(email: &str) -> ResetOptions {
    ResetOptions {
        email: email.into(),
        new_password: NEW_PASSWORD.into(),
        revoke_passkeys: false,
        revoke_share_tokens: false,
    }
}

async fn login(app: &TestApp, email: &str, password: &str) -> Reply {
    post(
        app,
        "/api/login",
        json!({ "email": email, "password": password }),
        None,
    )
    .await
}

#[tokio::test]
async fn reset_sets_the_password_and_ends_sessions() {
    let app = setup(true).await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let other = register_cookie(&app, "b@example.org").await;

    let report = cli::reset_credentials(&app.pool, &options("  A@Example.org "))
        .await
        .unwrap();
    assert_eq!(
        report,
        ResetReport {
            sessions_ended: 1,
            passkeys_removed: 0,
            share_tokens_removed: 0
        }
    );

    // Alte Sitzung ist sofort ungültig, das alte Passwort auch, das neue gilt.
    assert_eq!(
        get(&app, "/api/me", Some(&cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "a@example.org", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "a@example.org", NEW_PASSWORD).await.status,
        StatusCode::OK
    );
    // Andere Konten bleiben unberührt.
    assert_eq!(
        get(&app, "/api/me", Some(&other)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        login(&app, "b@example.org", PASSWORD).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn reset_keeps_passkeys_and_tokens_unless_asked() {
    let app = setup_with(Config {
        registration_enabled: true,
        passkey_origin: Some("https://lesen.test".into()),
        ..test_config()
    })
    .await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = SoftAuthenticator::new("https://lesen.test", "lesen.test");
    let start = post(
        &app,
        "/api/passkeys/register/start",
        json!({ "name": "iPhone", "password": PASSWORD }),
        Some(&cookie),
    )
    .await;
    let credential = phone.register(&start.body);
    post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(&cookie),
    )
    .await;
    post(
        &app,
        "/api/share-tokens",
        json!({ "name": "Pixel" }),
        Some(&cookie),
    )
    .await;

    let kept = cli::reset_credentials(&app.pool, &options("a@example.org"))
        .await
        .unwrap();
    assert_eq!((kept.passkeys_removed, kept.share_tokens_removed), (0, 0));
    assert_eq!(count(&app.pool, "passkeys").await, 1);
    let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM share_tokens")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(tokens, 1);

    let revoked = cli::reset_credentials(
        &app.pool,
        &ResetOptions {
            revoke_passkeys: true,
            revoke_share_tokens: true,
            ..options("a@example.org")
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (revoked.passkeys_removed, revoked.share_tokens_removed),
        (1, 1)
    );
    assert_eq!(count(&app.pool, "passkeys").await, 0);
}

#[tokio::test]
async fn reset_rejects_unknown_users_and_weak_passwords() {
    let app = setup(true).await;
    register_cookie(&app, "a@example.org").await;

    let unknown = cli::reset_credentials(&app.pool, &options("niemand@example.org")).await;
    assert!(matches!(unknown, Err(ResetError::UnknownUser)));

    let weak = cli::reset_credentials(
        &app.pool,
        &ResetOptions {
            new_password: "kurz".into(),
            ..options("a@example.org")
        },
    )
    .await;
    assert!(matches!(weak, Err(ResetError::InvalidPassword(_))));
    // Nichts wurde geändert.
    assert_eq!(
        login(&app, "a@example.org", PASSWORD).await.status,
        StatusCode::OK
    );
}
