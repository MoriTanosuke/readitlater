//! Registrierung, Login, Logout, Konto löschen.

use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{StatusCode, request::Parts},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    error::{ApiError, ApiJson},
};

const COOKIE_NAME: &str = "session";
const MIN_PASSWORD_LEN: usize = 10;
const MAX_PASSWORD_LEN: usize = 256;
const MAX_EMAIL_LEN: usize = 254;

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct PasswordConfirmation {
    password: String,
}

#[derive(Serialize)]
pub struct UserOut {
    id: i64,
    email: String,
}

/// Angemeldeter Benutzer (Extraktor, liest das Session-Cookie).
pub struct AuthUser {
    pub id: i64,
    pub email: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(COOKIE_NAME)
            .map(|c| c.value().to_owned())
            .ok_or(ApiError::Unauthorized)?;
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT u.id, u.email FROM sessions s \
             JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = ? AND s.expires_at > ?",
        )
        .bind(token_hash(&token))
        .bind(now_secs())
        .fetch_optional(&state.pool)
        .await?;
        match row {
            Some((id, email)) => Ok(AuthUser { id, email }),
            None => Err(ApiError::Unauthorized),
        }
    }
}

pub async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(creds): ApiJson<Credentials>,
) -> Result<(StatusCode, CookieJar, Json<UserOut>), ApiError> {
    if !state.config.registration_enabled {
        return Err(ApiError::Forbidden(
            "Die Registrierung ist deaktiviert".into(),
        ));
    }
    let email = normalize_email(&creds.email)?;
    validate_password(&creds.password)?;

    let password = creds.password;
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(ApiError::internal)??;

    let inserted = sqlx::query("INSERT INTO users (email, password_hash) VALUES (?, ?)")
        .bind(&email)
        .bind(&hash)
        .execute(&state.pool)
        .await;
    let id = match inserted {
        Ok(result) => result.last_insert_rowid(),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::Conflict(
                "Diese E-Mail ist bereits registriert".into(),
            ));
        }
        Err(e) => return Err(e.into()),
    };

    let token = create_session(&state, id).await?;
    let jar = jar.add(session_cookie(&state, token));
    Ok((StatusCode::CREATED, jar, Json(UserOut { id, email })))
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(creds): ApiJson<Credentials>,
) -> Result<(CookieJar, Json<UserOut>), ApiError> {
    let email = creds.email.trim().to_lowercase();
    let user: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, email, password_hash FROM users WHERE email = ?")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await?;

    // Auch bei unbekannter E-Mail wird ein Hash geprüft, damit die Antwortzeit
    // nichts über die Existenz eines Kontos verrät.
    let (stored_hash, known) = match &user {
        Some((_, _, hash)) => (hash.clone(), true),
        None => (dummy_hash().to_owned(), false),
    };
    let password = creds.password;
    let valid = tokio::task::spawn_blocking(move || verify_password(&password, &stored_hash))
        .await
        .map_err(ApiError::internal)?;

    let (id, email) = match user {
        Some((id, email, _)) if known && valid => (id, email),
        _ => return Err(ApiError::BadCredentials),
    };

    purge_expired_sessions(&state).await;
    let token = create_session(&state, id).await?;
    let jar = jar.add(session_cookie(&state, token));
    Ok((jar, Json(UserOut { id, email })))
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    if let Some(cookie) = jar.get(COOKIE_NAME) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash(cookie.value()))
            .execute(&state.pool)
            .await?;
    }
    Ok((StatusCode::NO_CONTENT, jar.remove(removal_cookie())))
}

pub async fn me(user: AuthUser) -> Json<UserOut> {
    Json(UserOut {
        id: user.id,
        email: user.email,
    })
}

/// Löscht das Konto samt Sessions, Artikeln und Schlagwörtern (ON DELETE CASCADE).
/// Zur Sicherheit muss das Passwort erneut angegeben werden.
pub async fn delete_account(
    State(state): State<AppState>,
    user: AuthUser,
    jar: CookieJar,
    ApiJson(confirmation): ApiJson<PasswordConfirmation>,
) -> Result<(StatusCode, CookieJar), ApiError> {
    let stored_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
    let password = confirmation.password;
    let valid = tokio::task::spawn_blocking(move || verify_password(&password, &stored_hash))
        .await
        .map_err(ApiError::internal)?;
    if !valid {
        return Err(ApiError::Forbidden("Passwort ist falsch".into()));
    }

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok((StatusCode::NO_CONTENT, jar.remove(removal_cookie())))
}

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

fn normalize_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_lowercase();
    let valid = email.len() <= MAX_EMAIL_LEN
        && !email.contains(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if valid {
        Ok(email)
    } else {
        Err(ApiError::BadRequest("Ungültige E-Mail-Adresse".into()))
    }
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(ApiError::BadRequest(format!(
            "Das Passwort muss mindestens {MIN_PASSWORD_LEN} Zeichen lang sein"
        )));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(ApiError::BadRequest(format!(
            "Das Passwort darf höchstens {MAX_PASSWORD_LEN} Zeichen lang sein"
        )));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, ApiError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(ApiError::internal)
}

fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("dummy-passwort-fuer-timing").unwrap_or_default())
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn create_session(state: &AppState, user_id: i64) -> Result<String, ApiError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    let token = hex::encode(bytes);
    let expires_at = now_secs() + state.config.session_days * 86_400;
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(token_hash(&token))
        .bind(user_id)
        .bind(expires_at)
        .execute(&state.pool)
        .await?;
    Ok(token)
}

/// Räumt abgelaufene Sessions auf (Fehler sind nicht kritisch).
pub async fn purge_expired_sessions(state: &AppState) {
    if let Err(e) = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now_secs())
        .execute(&state.pool)
        .await
    {
        tracing::warn!("Aufräumen der Sessions fehlgeschlagen: {e}");
    }
}

fn session_cookie(state: &AppState, token: String) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.cookie_secure)
        .max_age(time::Duration::days(state.config.session_days))
        .build()
}

fn removal_cookie() -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, "")).path("/").build()
}
