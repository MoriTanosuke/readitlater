//! Artikel speichern, auflisten, anzeigen und löschen.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    AppState,
    auth::AuthUser,
    error::{ApiError, ApiJson},
    extract::{self, ExtractError},
    fetch::FetchError,
};

const MAX_URL_LEN: usize = 2048;
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Deserialize)]
pub struct NewArticle {
    url: String,
}

#[derive(Deserialize)]
pub struct ListParams {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// Kurzform für die Liste (ohne den vollen Text).
#[derive(Serialize, sqlx::FromRow)]
pub struct ArticleSummary {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    is_read: bool,
    created_at: String,
}

/// Einzelner Artikel mit bereinigtem HTML-Inhalt.
#[derive(Serialize, sqlx::FromRow)]
pub struct ArticleFull {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    content: String,
    is_read: bool,
    created_at: String,
}

/// `POST /api/articles`: Seite laden, Hauptinhalt extrahieren und speichern.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<NewArticle>,
) -> Result<(StatusCode, Json<ArticleSummary>), ApiError> {
    let url = parse_url(&body.url)?;
    state.fetcher.validate(&url).map_err(fetch_error)?;
    let normalized = url.as_str().to_owned();

    // Bekannte Adressen gar nicht erst laden.
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM articles WHERE user_id = ? AND url = ?")
            .bind(user.id)
            .bind(&normalized)
            .fetch_optional(&state.pool)
            .await?;
    if existing.is_some() {
        return Err(already_saved());
    }

    let page = {
        let _slot = state
            .fetch_slots
            .try_acquire()
            .map_err(|_| ApiError::TooManyRequests)?;
        state.fetcher.fetch(&url).await.map_err(fetch_error)?
    };

    let final_url = page.final_url;
    let html = page.html;
    let extracted = tokio::task::spawn_blocking(move || extract::extract(&html, &final_url))
        .await
        .map_err(ApiError::internal)?
        .map_err(extract_error)?;

    let inserted = sqlx::query_as::<_, ArticleSummary>(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, url, title, excerpt, is_read, created_at",
    )
    .bind(user.id)
    .bind(&normalized)
    .bind(&extracted.title)
    .bind(&extracted.content_html)
    .bind(&extracted.content_text)
    .bind(&extracted.excerpt)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(article) => Ok((StatusCode::CREATED, Json(article))),
        // Zwei gleichzeitige Anfragen für dieselbe Adresse.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(already_saved()),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/articles?limit=&offset=`: eigene Artikel, neueste zuerst.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ArticleSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);
    let articles = sqlx::query_as::<_, ArticleSummary>(
        "SELECT id, url, title, excerpt, is_read, created_at FROM articles \
         WHERE user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(articles))
}

/// `GET /api/articles/{id}`: ein eigener Artikel mit Inhalt.
pub async fn show(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<ArticleFull>, ApiError> {
    let article = sqlx::query_as::<_, ArticleFull>(
        "SELECT id, url, title, excerpt, content, is_read, created_at FROM articles \
         WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    article.map(Json).ok_or_else(not_found)
}

/// `DELETE /api/articles/{id}`: eigenen Artikel löschen (Schlagwort-Zuordnungen per CASCADE).
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("DELETE FROM articles WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Eingabe zu einer URL aufbereiten: ohne Schema wird `https://` ergänzt,
/// das Fragment (`#...`) entfällt, damit dieselbe Seite nicht doppelt gespeichert wird.
fn parse_url(input: &str) -> Result<Url, ApiError> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LEN {
        return Err(invalid_url());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|_| invalid_url())?;
    url.set_fragment(None);
    Ok(url)
}

fn invalid_url() -> ApiError {
    ApiError::BadRequest("Ungültige Adresse. Erlaubt sind http- und https-Adressen".into())
}

fn already_saved() -> ApiError {
    ApiError::Conflict("Dieser Artikel ist bereits gespeichert".into())
}

fn not_found() -> ApiError {
    ApiError::NotFound("Artikel nicht gefunden".into())
}

fn fetch_error(err: FetchError) -> ApiError {
    match err {
        FetchError::InvalidUrl => invalid_url(),
        FetchError::Blocked => ApiError::BadRequest("Diese Adresse ist nicht erlaubt".into()),
        FetchError::Timeout => {
            ApiError::BadGateway("Die Seite hat nicht rechtzeitig geantwortet".into())
        }
        FetchError::TooLarge => {
            ApiError::Unprocessable("Die Seite ist zu groß zum Speichern".into())
        }
        FetchError::NotHtml => {
            ApiError::Unprocessable("Die Adresse liefert keine Webseite (HTML)".into())
        }
        FetchError::Status(code) => {
            ApiError::BadGateway(format!("Die Seite antwortete mit Status {}", code.as_u16()))
        }
        FetchError::TooManyRedirects => {
            ApiError::BadGateway("Die Seite leitet zu oft weiter".into())
        }
        FetchError::Upstream(detail) => {
            tracing::info!("Seitenabruf fehlgeschlagen: {detail}");
            ApiError::BadGateway("Die Seite konnte nicht geladen werden".into())
        }
    }
}

fn extract_error(err: ExtractError) -> ApiError {
    match err {
        ExtractError::NoContent => {
            ApiError::Unprocessable("Auf der Seite wurde kein lesbarer Artikeltext gefunden".into())
        }
        ExtractError::TooComplex => {
            ApiError::Unprocessable("Die Seite ist zu umfangreich zum Auswerten".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str) -> String {
        parse_url(input).unwrap().to_string()
    }

    #[test]
    fn fehlendes_schema_wird_zu_https() {
        assert_eq!(parsed("example.com/artikel"), "https://example.com/artikel");
        assert_eq!(parsed("  example.com  "), "https://example.com/");
    }

    #[test]
    fn schema_bleibt_erhalten_und_fragment_entfaellt() {
        assert_eq!(parsed("http://example.com/a#teil"), "http://example.com/a");
        assert_eq!(
            parsed("HTTPS://Example.COM/A?x=1"),
            "https://example.com/A?x=1"
        );
    }

    #[test]
    fn unbrauchbare_eingaben_werden_abgelehnt() {
        for input in [
            "",
            "   ",
            "javascript:alert(1)",
            "http://",
            "https://exa mple.com",
        ] {
            assert!(parse_url(input).is_err(), "{input:?}");
        }
        assert!(parse_url(&format!("example.com/{}", "a".repeat(MAX_URL_LEN))).is_err());
    }
}
