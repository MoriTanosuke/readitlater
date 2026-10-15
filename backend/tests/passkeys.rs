//! Integrationstests für Passkeys (WebAuthn): Einrichten, Anmelden ohne E-Mail, Verwalten.
//! Ein Test-Authenticator (`common::authenticator`) spielt das Gerät.

mod common;

use axum::http::{Method, StatusCode};
use common::{authenticator::SoftAuthenticator, *};
use readlater_backend::config::Config;
use serde_json::{Value, json};

const ORIGIN: &str = "https://lesen.test";
const RP_ID: &str = "lesen.test";

async fn setup_passkeys() -> TestApp {
    setup_with(Config {
        registration_enabled: true,
        passkey_origin: Some(ORIGIN.into()),
        ..test_config()
    })
    .await
}

fn device() -> SoftAuthenticator {
    SoftAuthenticator::new(ORIGIN, RP_ID)
}

async fn start_registration(app: &TestApp, cookie: &str, name: &str, password: &str) -> Reply {
    post(
        app,
        "/api/passkeys/register/start",
        json!({ "name": name, "password": password }),
        Some(cookie),
    )
    .await
}

/// Richtet einen Passkey ein (Start, Gerät antwortet, Abschluss) und gibt die Antwort des Abschlusses zurück.
async fn add_passkey(
    app: &TestApp,
    cookie: &str,
    name: &str,
    device: &mut SoftAuthenticator,
) -> Reply {
    let start = start_registration(app, cookie, name, PASSWORD).await;
    assert_eq!(start.status, StatusCode::OK, "Start: {}", start.error());
    let credential = device.register(&start.body);
    post(
        app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(cookie),
    )
    .await
}

/// Anmeldung ohne E-Mail: Start, Gerät antwortet, Abschluss.
async fn login_with(app: &TestApp, device: &mut SoftAuthenticator) -> Reply {
    let start = post(app, "/api/passkeys/login/start", json!({}), None).await;
    assert_eq!(start.status, StatusCode::OK, "Start: {}", start.error());
    let id = start.body["id"].as_str().expect("id").to_string();
    let credential = device.authenticate(&start.body["options"]);
    post(
        app,
        "/api/passkeys/login/finish",
        json!({ "id": id, "credential": credential }),
        None,
    )
    .await
}

async fn list_passkeys(app: &TestApp, cookie: &str) -> Vec<Value> {
    let reply = get(app, "/api/passkeys", Some(cookie)).await;
    assert_eq!(reply.status, StatusCode::OK);
    reply.body.as_array().expect("Liste").clone()
}

#[tokio::test]
async fn passkeys_are_off_without_configuration() {
    let app = setup(true).await;
    let features = get(&app, "/api/features", None).await;
    assert_eq!(features.status, StatusCode::OK);
    assert_eq!(features.body["passkeys"], false);

    let cookie = register_cookie(&app, "a@example.org").await;
    let start = start_registration(&app, &cookie, "iPhone", PASSWORD).await;
    assert_eq!(start.status, StatusCode::NOT_FOUND);
    let login = post(&app, "/api/passkeys/login/start", json!({}), None).await;
    assert_eq!(login.status, StatusCode::NOT_FOUND);
    // Die Liste bleibt abrufbar (leer).
    assert!(list_passkeys(&app, &cookie).await.is_empty());
}

#[tokio::test]
async fn features_report_passkeys_when_configured() {
    let app = setup_passkeys().await;
    assert_eq!(
        get(&app, "/api/features", None).await.body["passkeys"],
        true
    );
}

#[tokio::test]
async fn register_and_log_in_without_email() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();

    let finished = add_passkey(&app, &cookie, "  Mein   iPhone ", &mut phone).await;
    assert_eq!(finished.status, StatusCode::CREATED, "{}", finished.error());
    assert_eq!(finished.body["name"], "Mein iPhone");
    assert!(finished.body["last_used_at"].is_null());
    // Die Antwort enthält kein Schlüsselmaterial.
    assert!(finished.body.get("passkey").is_none());
    assert!(finished.body.get("credential_id").is_none());
    assert_eq!(count(&app.pool, "passkeys").await, 1);

    // Anmelden ohne bestehende Sitzung und ohne E-Mail.
    let login = login_with(&app, &mut phone).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.error());
    assert_eq!(login.body["email"], "a@example.org");
    let session = login.cookie().expect("Session-Cookie");
    assert!(login.set_cookie_raw().contains("HttpOnly"));
    let me = get(&app, "/api/me", Some(&session)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["email"], "a@example.org");

    // Die letzte Nutzung ist jetzt vermerkt.
    let passkeys = list_passkeys(&app, &cookie).await;
    assert_eq!(passkeys.len(), 1);
    assert!(passkeys[0]["last_used_at"].is_string());

    // Ein zweites Mal geht ebenfalls (der Zähler im Gerät steigt).
    let again = login_with(&app, &mut phone).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.error());
}

#[tokio::test]
async fn registration_asks_for_a_discoverable_passkey() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let start = start_registration(&app, &cookie, "iPhone", PASSWORD).await;
    assert_eq!(start.status, StatusCode::OK);
    let public_key = &start.body["publicKey"];
    assert_eq!(public_key["rp"]["id"], RP_ID);
    assert_eq!(public_key["user"]["name"], "a@example.org");
    let selection = &public_key["authenticatorSelection"];
    assert_eq!(selection["residentKey"], "required");
    assert_eq!(selection["requireResidentKey"], true);
    assert_eq!(selection["userVerification"], "required");
}

#[tokio::test]
async fn registration_needs_password_and_csrf_header() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;

    let wrong = start_registration(&app, &cookie, "iPhone", "falsches-passwort").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    assert_eq!(wrong.error(), "Passwort ist falsch");

    let no_session = post(
        &app,
        "/api/passkeys/register/start",
        json!({ "name": "iPhone", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(no_session.status, StatusCode::UNAUTHORIZED);

    let no_csrf = call(
        &app,
        Method::POST,
        "/api/passkeys/register/start",
        Some(json!({ "name": "iPhone", "password": PASSWORD })),
        Some(&cookie),
        false,
    )
    .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);

    for name in ["", "   ", &"x".repeat(61)] {
        let bad = start_registration(&app, &cookie, name, PASSWORD).await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST, "Name {name:?}");
    }
}

#[tokio::test]
async fn finishing_without_start_or_by_another_user_fails() {
    let app = setup_passkeys().await;
    let alice = register_cookie(&app, "a@example.org").await;
    let bob = register_cookie(&app, "b@example.org").await;
    let mut phone = device();

    let start = start_registration(&app, &alice, "iPhone", PASSWORD).await;
    let credential = phone.register(&start.body);

    // Bob hat nichts angefangen: sein Abschluss wird abgelehnt.
    let stolen = post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(&bob),
    )
    .await;
    assert_eq!(stolen.status, StatusCode::BAD_REQUEST);

    // Alice' angefangene Einrichtung bleibt gültig, genau einmal.
    let ok = post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(&alice),
    )
    .await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.error());
    let replay = post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(&alice),
    )
    .await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&app.pool, "passkeys").await, 1);
}

#[tokio::test]
async fn registration_from_a_foreign_origin_is_rejected() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phishing = SoftAuthenticator::new("https://lesen-evil.test", RP_ID);
    let finished = add_passkey(&app, &cookie, "iPhone", &mut phishing).await;
    assert_eq!(finished.status, StatusCode::BAD_REQUEST);
    assert_eq!(count(&app.pool, "passkeys").await, 0);
}

#[tokio::test]
async fn login_from_a_foreign_origin_is_rejected() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();
    add_passkey(&app, &cookie, "iPhone", &mut phone).await;

    // Gleiche Schlüssel, aber der Browser meldet eine andere Herkunft (Phishing-Seite).
    phone.origin = "https://lesen-evil.test".into();
    let login = login_with(&app, &mut phone).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    assert_eq!(login.error(), "Anmeldung mit Passkey fehlgeschlagen");
    assert!(login.cookie().is_none());
}

#[tokio::test]
async fn login_challenge_works_only_once() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();
    add_passkey(&app, &cookie, "iPhone", &mut phone).await;

    let start = post(&app, "/api/passkeys/login/start", json!({}), None).await;
    let id = start.body["id"].as_str().unwrap().to_string();
    let credential = phone.authenticate(&start.body["options"]);
    let body = json!({ "id": id, "credential": credential });

    let first = post(&app, "/api/passkeys/login/finish", body.clone(), None).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.error());
    // Eine abgefangene Antwort lässt sich nicht noch einmal einspielen.
    let replay = post(&app, "/api/passkeys/login/finish", body, None).await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);

    // Unbekannte Kennung
    let credential = phone.authenticate(&start.body["options"]);
    let unknown = post(
        &app,
        "/api/passkeys/login/finish",
        json!({ "id": "0123456789abcdef0123456789abcdef", "credential": credential }),
        None,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unregistered_device_cannot_log_in() {
    let app = setup_passkeys().await;
    register_cookie(&app, "a@example.org").await;
    // Dieses Gerät hat nie einen Passkey eingerichtet, sein Schlüssel ist dem Server unbekannt.
    let mut stranger = device();
    let fake_start = post(&app, "/api/passkeys/login/start", json!({}), None).await;
    let id = fake_start.body["id"].as_str().unwrap().to_string();
    let credential = stranger.authenticate(&fake_start.body["options"]);
    let login = post(
        &app,
        "/api/passkeys/login/finish",
        json!({ "id": id, "credential": credential }),
        None,
    )
    .await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn malformed_login_body_is_a_bad_request() {
    let app = setup_passkeys().await;
    let reply = post(
        &app,
        "/api/passkeys/login/finish",
        json!({ "id": "x", "credential": { "id": 5 } }),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn each_user_logs_in_as_themselves() {
    let app = setup_passkeys().await;
    let alice = register_cookie(&app, "a@example.org").await;
    let bob = register_cookie(&app, "b@example.org").await;
    let mut alice_phone = device();
    let mut bob_phone = device();
    add_passkey(&app, &alice, "Alice", &mut alice_phone).await;
    add_passkey(&app, &bob, "Bob", &mut bob_phone).await;

    assert_eq!(
        login_with(&app, &mut bob_phone).await.body["email"],
        "b@example.org"
    );
    assert_eq!(
        login_with(&app, &mut alice_phone).await.body["email"],
        "a@example.org"
    );
    // Jeder sieht nur die eigenen Passkeys.
    assert_eq!(list_passkeys(&app, &alice).await.len(), 1);
    assert_eq!(list_passkeys(&app, &alice).await[0]["name"], "Alice");
}

#[tokio::test]
async fn several_passkeys_share_one_user_handle_and_exclude_each_other() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();
    let mut laptop = device();

    let first = start_registration(&app, &cookie, "iPhone", PASSWORD).await;
    assert!(first.body["publicKey"]["excludeCredentials"].is_null());
    let credential = phone.register(&first.body);
    let created = post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential.clone() }),
        Some(&cookie),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let second = start_registration(&app, &cookie, "Laptop", PASSWORD).await;
    assert_eq!(
        second.body["publicKey"]["user"]["id"], first.body["publicKey"]["user"]["id"],
        "gleiche Kennung für alle Passkeys eines Benutzers"
    );
    let excluded = second.body["publicKey"]["excludeCredentials"]
        .as_array()
        .expect("excludeCredentials");
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0]["id"], credential["id"]);
    let credential = laptop.register(&second.body);
    let created = post(
        &app,
        "/api/passkeys/register/finish",
        json!({ "credential": credential }),
        Some(&cookie),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.error());
    assert_eq!(list_passkeys(&app, &cookie).await.len(), 2);

    // Beide Geräte melden dasselbe Konto an.
    assert_eq!(
        login_with(&app, &mut phone).await.body["email"],
        "a@example.org"
    );
    assert_eq!(
        login_with(&app, &mut laptop).await.body["email"],
        "a@example.org"
    );
}

#[tokio::test]
async fn at_most_ten_passkeys_per_user() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    for index in 0..10 {
        let finished = add_passkey(&app, &cookie, &format!("Gerät {index}"), &mut device()).await;
        assert_eq!(finished.status, StatusCode::CREATED, "Gerät {index}");
    }
    let eleventh = start_registration(&app, &cookie, "Zu viel", PASSWORD).await;
    assert_eq!(eleventh.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count(&app.pool, "passkeys").await, 10);
}

#[tokio::test]
async fn rename_validates_and_is_limited_to_the_owner() {
    let app = setup_passkeys().await;
    let alice = register_cookie(&app, "a@example.org").await;
    let bob = register_cookie(&app, "b@example.org").await;
    let created = add_passkey(&app, &alice, "iPhone", &mut device()).await;
    let id = created.body["id"].as_i64().unwrap();
    let uri = format!("/api/passkeys/{id}");

    let renamed = call(
        &app,
        Method::PATCH,
        &uri,
        Some(json!({ "name": "  Neues   Handy " })),
        Some(&alice),
        true,
    )
    .await;
    assert_eq!(renamed.status, StatusCode::OK);
    assert_eq!(renamed.body["name"], "Neues Handy");

    let empty = call(
        &app,
        Method::PATCH,
        &uri,
        Some(json!({ "name": " " })),
        Some(&alice),
        true,
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let foreign = call(
        &app,
        Method::PATCH,
        &uri,
        Some(json!({ "name": "Meins" })),
        Some(&bob),
        true,
    )
    .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    assert_eq!(list_passkeys(&app, &alice).await[0]["name"], "Neues Handy");
}

#[tokio::test]
async fn delete_needs_the_password_and_only_works_for_the_owner() {
    let app = setup_passkeys().await;
    let alice = register_cookie(&app, "a@example.org").await;
    let bob = register_cookie(&app, "b@example.org").await;
    let mut phone = device();
    let created = add_passkey(&app, &alice, "iPhone", &mut phone).await;
    let uri = format!("/api/passkeys/{}", created.body["id"].as_i64().unwrap());

    let wrong = call(
        &app,
        Method::DELETE,
        &uri,
        Some(json!({ "password": "falsches-passwort" })),
        Some(&alice),
        true,
    )
    .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app.pool, "passkeys").await, 1);

    // Bob kennt sein eigenes Passwort, aber der Passkey gehört ihm nicht.
    let foreign = call(
        &app,
        Method::DELETE,
        &uri,
        Some(json!({ "password": PASSWORD })),
        Some(&bob),
        true,
    )
    .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND);
    assert_eq!(count(&app.pool, "passkeys").await, 1);

    let deleted = call(
        &app,
        Method::DELETE,
        &uri,
        Some(json!({ "password": PASSWORD })),
        Some(&alice),
        true,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(count(&app.pool, "passkeys").await, 0);

    // Der gelöschte Passkey meldet nicht mehr an, das Passwort aber weiterhin.
    let login = login_with(&app, &mut phone).await;
    assert_eq!(login.status, StatusCode::UNAUTHORIZED);
    let password_login = post(
        &app,
        "/api/login",
        json!({ "email": "a@example.org", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(password_login.status, StatusCode::OK);
}

#[tokio::test]
async fn deleting_the_account_removes_its_passkeys() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    add_passkey(&app, &cookie, "iPhone", &mut device()).await;
    assert_eq!(count(&app.pool, "passkeys").await, 1);
    let deleted = call(
        &app,
        Method::DELETE,
        "/api/account",
        Some(json!({ "password": PASSWORD })),
        Some(&cookie),
        true,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(count(&app.pool, "passkeys").await, 0);
}

#[tokio::test]
async fn changing_the_password_keeps_passkeys_working() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();
    add_passkey(&app, &cookie, "iPhone", &mut phone).await;
    let changed = call(
        &app,
        Method::PUT,
        "/api/account/password",
        Some(json!({ "current_password": PASSWORD, "new_password": "ein-ganz-neues-passwort" })),
        Some(&cookie),
        true,
    )
    .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT);
    let login = login_with(&app, &mut phone).await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.error());
}

#[tokio::test]
async fn stored_passkey_contains_no_secret_and_credential_is_unique() {
    let app = setup_passkeys().await;
    let cookie = register_cookie(&app, "a@example.org").await;
    let mut phone = device();
    add_passkey(&app, &cookie, "iPhone", &mut phone).await;
    let (credential_id, handle, serialized): (Vec<u8>, Vec<u8>, String) =
        sqlx::query_as("SELECT credential_id, user_handle, passkey FROM passkeys")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(credential_id.len(), 32);
    assert_eq!(handle.len(), 16);
    // Der Handle ist keine Zahl aus der Benutzertabelle und verrät weder E-Mail noch ID.
    assert!(!String::from_utf8_lossy(&handle).contains("example"));
    assert!(serde_json::from_str::<Value>(&serialized).is_ok());
    assert!(!serialized.contains("a@example.org"));

    // Dieselbe Credential-ID lässt sich nicht noch einmal ablegen.
    let duplicate = sqlx::query(
        "INSERT INTO passkeys (user_id, name, credential_id, user_handle, passkey) \
         VALUES (1, 'doppelt', ?, ?, '{}')",
    )
    .bind(credential_id)
    .bind(handle)
    .execute(&app.pool)
    .await;
    assert!(duplicate.is_err());
}
