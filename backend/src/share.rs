//! Artikel aus anderen Apps teilen (Android: HTTP Shortcuts, iPad/iPhone: Kurzbefehle).
//!
//! Dafür gibt es eigene Tokens: Sie werden in der Weboberfläche erzeugt, nur als
//! SHA-256-Hash gespeichert und dürfen ausschließlich `POST /api/share` aufrufen.
//! Ein gestohlenes Token kann also Artikel hinzufügen, aber nichts lesen oder löschen,
//! und es lässt sich einzeln widerrufen. Da die Anfrage nur den Header `Authorization`
//! und kein Cookie auswertet, ist kein CSRF-Schutz nötig.

use axum::{
    Json,
    body::Bytes,
    extract::{FromRequestParts, Path, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    articles::{self, ArticleSummary},
    auth::{AuthUser, token_hash},
    error::ApiError,
};

const TOKEN_PREFIX: &str = "lsl_";
const MAX_TOKENS_PER_USER: i64 = 10;
const MAX_NAME_LEN: usize = 60;

/// Benutzer, der sich mit einem Share-Token ausweist (Header `Authorization: Bearer ...`).
pub struct ShareUser {
    id: i64,
}

impl FromRequestParts<AppState> for ShareUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let user_id: Option<i64> = sqlx::query_scalar(
            "UPDATE share_tokens SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
             WHERE token_hash = ? RETURNING user_id",
        )
        .bind(token_hash(token))
        .fetch_optional(&state.pool)
        .await?;
        user_id
            .map(|id| ShareUser { id })
            .ok_or(ApiError::Unauthorized)
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

#[derive(Deserialize)]
struct JsonBody {
    url: Option<String>,
    text: Option<String>,
}

/// Holt die Adresse aus dem Body: JSON (`url` oder `text`), Formular (`url` oder `text`)
/// oder reiner Text. Die Share-Funktion von Android liefert oft "Titel https://...",
/// deshalb zählt die erste http(s)-Adresse im Text.
fn url_from_body(headers: &HeaderMap, body: &[u8]) -> Result<String, ApiError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = if content_type.starts_with("application/json") {
        let parsed: JsonBody = serde_json::from_slice(body)
            .map_err(|_| ApiError::BadRequest("Ungültige Anfrage: kein gültiges JSON".into()))?;
        parsed.url.or(parsed.text)
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        let mut url = None;
        let mut text = None;
        for (key, value) in url::form_urlencoded::parse(body) {
            match key.as_ref() {
                "url" => url = Some(value.into_owned()),
                "text" => text = Some(value.into_owned()),
                _ => {}
            }
        }
        url.or(text)
    } else {
        Some(String::from_utf8_lossy(body).into_owned())
    };
    let text = text.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Es wurde keine Adresse übergeben (Feld url oder text)".into(),
        ));
    }
    Ok(find_url(&text))
}

/// Erste Angabe mit `http://` oder `https://`, sonst der ganze Text.
fn find_url(text: &str) -> String {
    text.split_whitespace()
        .find(|word| {
            let lower = word.to_ascii_lowercase();
            lower.starts_with("http://") || lower.starts_with("https://")
        })
        .unwrap_or_else(|| text.trim())
        .to_owned()
}

/// `POST /api/share`: Artikel speichern, Authentifizierung per Share-Token.
pub async fn share(
    State(state): State<AppState>,
    user: ShareUser,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<ArticleSummary>), ApiError> {
    let url = url_from_body(&headers, &body)?;
    let summary = articles::save_article(&state, user.id, &url).await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

#[derive(Deserialize)]
pub struct NewToken {
    name: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TokenInfo {
    id: i64,
    name: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Serialize)]
pub struct CreatedToken {
    #[serde(flatten)]
    info: TokenInfo,
    /// Wird nur hier einmalig angezeigt.
    token: String,
}

/// `GET /api/share-tokens`: eigene Tokens (ohne den Wert).
pub async fn list_tokens(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TokenInfo>>, ApiError> {
    let tokens = sqlx::query_as::<_, TokenInfo>(
        "SELECT id, name, created_at, last_used_at FROM share_tokens \
         WHERE user_id = ? ORDER BY id",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(tokens))
}

/// `POST /api/share-tokens`: neues Token erzeugen. Der Wert steht nur in dieser Antwort.
pub async fn create_token(
    State(state): State<AppState>,
    user: AuthUser,
    crate::error::ApiJson(body): crate::error::ApiJson<NewToken>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    let name = body.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest(format!(
            "Der Name muss 1 bis {MAX_NAME_LEN} Zeichen lang sein"
        )));
    }
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM share_tokens WHERE user_id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
    if existing >= MAX_TOKENS_PER_USER {
        return Err(ApiError::Unprocessable(format!(
            "Es sind höchstens {MAX_TOKENS_PER_USER} Tokens möglich. Bitte zuerst eines löschen"
        )));
    }

    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    let token = format!("{TOKEN_PREFIX}{}", hex::encode(bytes));
    let info = sqlx::query_as::<_, TokenInfo>(
        "INSERT INTO share_tokens (user_id, name, token_hash) VALUES (?, ?, ?) \
         RETURNING id, name, created_at, last_used_at",
    )
    .bind(user.id)
    .bind(&name)
    .bind(token_hash(&token))
    .fetch_one(&state.pool)
    .await?;
    tracing::info!("Share-Token {} für Benutzer {} angelegt", info.id, user.id);
    Ok((StatusCode::CREATED, Json(CreatedToken { info, token })))
}

/// `DELETE /api/share-tokens/{id}`: Token widerrufen.
pub async fn delete_token(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("DELETE FROM share_tokens WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Token nicht gefunden".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_url_in_shared_text() {
        assert_eq!(
            find_url("Spannender Artikel https://example.org/a?x=1 lesenswert"),
            "https://example.org/a?x=1"
        );
        assert_eq!(find_url("  example.org/a  "), "example.org/a");
        assert_eq!(find_url("HTTP://Example.org"), "HTTP://Example.org");
    }

    #[test]
    fn bearer_header_is_parsed() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, "bearer  abc ".parse().unwrap());
        assert_eq!(bearer(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(bearer(&h), None);
        h.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert_eq!(bearer(&h), None);
    }
}
