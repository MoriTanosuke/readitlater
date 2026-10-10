//! Artikel speichern, auflisten, anzeigen und löschen.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use url::Url;

use crate::{
    AppState,
    auth::AuthUser,
    error::{ApiError, ApiJson},
    extract::{self, ExtractError},
    fetch::FetchError,
    tags,
};

const MAX_URL_LEN: usize = 2048;
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_QUERY_LEN: usize = 200;
const MAX_QUERY_WORDS: usize = 10;

#[derive(Deserialize)]
pub struct NewArticle {
    url: String,
}

#[derive(Deserialize)]
pub struct ListParams {
    limit: Option<i64>,
    offset: Option<i64>,
    /// Suchtext (Volltext über Titel und Artikeltext).
    q: Option<String>,
    /// Nur Artikel mit diesem Schlagwort.
    tag: Option<String>,
    /// `true` = nur gelesene, `false` = nur ungelesene Artikel.
    read: Option<bool>,
}

/// Teilweise Änderung eines Artikels: Gelesen-Status und/oder Schlagwörter
/// (die Liste ersetzt die bisherigen Schlagwörter vollständig).
#[derive(Deserialize)]
pub struct ArticlePatch {
    is_read: Option<bool>,
    tags: Option<Vec<String>>,
}

/// Kurzform für die Liste (ohne den vollen Text).
#[derive(Serialize)]
pub struct ArticleSummary {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    /// Nur bei einer Suche: Textauszug, Treffer stehen zwischen \u{1} und \u{2}.
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    is_read: bool,
    created_at: String,
    tags: Vec<String>,
}

/// Einzelner Artikel mit bereinigtem HTML-Inhalt.
#[derive(Serialize)]
pub struct ArticleFull {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    content: String,
    is_read: bool,
    created_at: String,
    tags: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    snippet: Option<String>,
    is_read: bool,
    created_at: String,
    tags: String,
}

impl From<SummaryRow> for ArticleSummary {
    fn from(row: SummaryRow) -> Self {
        Self {
            id: row.id,
            url: row.url,
            title: row.title,
            excerpt: row.excerpt,
            snippet: row.snippet.filter(|s| !s.is_empty()),
            is_read: row.is_read,
            created_at: row.created_at,
            tags: parse_tags(&row.tags),
        }
    }
}

#[derive(sqlx::FromRow)]
struct FullRow {
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    content: String,
    is_read: bool,
    created_at: String,
    tags: String,
}

impl From<FullRow> for ArticleFull {
    fn from(row: FullRow) -> Self {
        Self {
            id: row.id,
            url: row.url,
            title: row.title,
            excerpt: row.excerpt,
            content: row.content,
            is_read: row.is_read,
            created_at: row.created_at,
            tags: parse_tags(&row.tags),
        }
    }
}

/// Schlagwörter kommen aus SQLite als JSON-Array (`json_group_array`).
fn parse_tags(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// `POST /api/articles`: Seite laden, Hauptinhalt extrahieren und speichern.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<NewArticle>,
) -> Result<(StatusCode, Json<ArticleSummary>), ApiError> {
    let summary = save_article(&state, user.id, &body.url).await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

/// Lädt die Seite, extrahiert den Hauptinhalt und speichert ihn für den Benutzer.
/// Gemeinsam genutzt von `POST /api/articles` und `POST /api/share`.
pub async fn save_article(
    state: &AppState,
    user_id: i64,
    raw_url: &str,
) -> Result<ArticleSummary, ApiError> {
    let url = parse_url(raw_url)?;
    state.fetcher.validate(&url).map_err(fetch_error)?;
    let normalized = url.as_str().to_owned();

    // Bekannte Adressen gar nicht erst laden.
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM articles WHERE user_id = ? AND url = ?")
            .bind(user_id)
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

    let inserted = sqlx::query_as::<_, SummaryRow>(
        "INSERT INTO articles (user_id, url, title, content, content_text, excerpt) \
         VALUES (?, ?, ?, ?, ?, ?) \
         RETURNING id, url, title, excerpt, NULL AS snippet, is_read, created_at, '[]' AS tags",
    )
    .bind(user_id)
    .bind(&normalized)
    .bind(&extracted.title)
    .bind(&extracted.content_html)
    .bind(&extracted.content_text)
    .bind(&extracted.excerpt)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(row) => Ok(row.into()),
        // Zwei gleichzeitige Anfragen für dieselbe Adresse.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(already_saved()),
        Err(e) => Err(e.into()),
    }
}

/// `GET /api/articles?q=&tag=&read=&limit=&offset=`: eigene Artikel.
/// Ohne Suchtext neueste zuerst, mit Suchtext nach Relevanz.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ArticleSummary>>, ApiError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);
    let tag = params
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let read = params.read;

    let rows = match search_query(params.q.as_deref())? {
        Some(fts) => {
            // Die Spalte tags liefert die Schlagwörter sortiert als JSON-Array.
            sqlx::query_as::<_, SummaryRow>(
                "SELECT a.id, a.url, a.title, a.excerpt, \
                        snippet(articles_fts, 1, char(1), char(2), '…', 24) AS snippet, \
                        a.is_read, a.created_at, \
                        (SELECT json_group_array(name) FROM ( \
                            SELECT t.name FROM article_tags at JOIN tags t ON t.id = at.tag_id \
                            WHERE at.article_id = a.id ORDER BY t.name)) AS tags \
                 FROM articles_fts \
                 JOIN articles a ON a.id = articles_fts.rowid \
                 WHERE articles_fts MATCH ?1 AND a.user_id = ?2 \
                   AND (?3 IS NULL OR a.is_read = ?3) \
                   AND (?4 IS NULL OR EXISTS (SELECT 1 FROM article_tags at \
                        JOIN tags t ON t.id = at.tag_id \
                        WHERE at.article_id = a.id AND t.name = ?4)) \
                 ORDER BY articles_fts.rank, a.id DESC LIMIT ?5 OFFSET ?6",
            )
            .bind(fts)
            .bind(user.id)
            .bind(read)
            .bind(tag)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, SummaryRow>(
                "SELECT a.id, a.url, a.title, a.excerpt, NULL AS snippet, \
                        a.is_read, a.created_at, \
                        (SELECT json_group_array(name) FROM ( \
                            SELECT t.name FROM article_tags at JOIN tags t ON t.id = at.tag_id \
                            WHERE at.article_id = a.id ORDER BY t.name)) AS tags \
                 FROM articles a \
                 WHERE a.user_id = ?1 \
                   AND (?2 IS NULL OR a.is_read = ?2) \
                   AND (?3 IS NULL OR EXISTS (SELECT 1 FROM article_tags at \
                        JOIN tags t ON t.id = at.tag_id \
                        WHERE at.article_id = a.id AND t.name = ?3)) \
                 ORDER BY a.id DESC LIMIT ?4 OFFSET ?5",
            )
            .bind(user.id)
            .bind(read)
            .bind(tag)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.pool)
            .await?
        }
    };
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// Macht aus dem Suchtext eine sichere FTS5-Abfrage: Jedes Wort wird als Phrase
/// in Anführungszeichen gesetzt (keine FTS-Operatoren von außen), das letzte Wort
/// matcht als Präfix („rust“ findet auch „rustacean“). Alle Wörter müssen vorkommen.
/// `None`, wenn kein Suchtext angegeben ist.
fn search_query(input: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(input) = input.map(str::trim).filter(|q| !q.is_empty()) else {
        return Ok(None);
    };
    if input.chars().count() > MAX_QUERY_LEN {
        return Err(ApiError::BadRequest(format!(
            "Der Suchtext darf höchstens {MAX_QUERY_LEN} Zeichen lang sein"
        )));
    }
    let words: Vec<&str> = input.split_whitespace().take(MAX_QUERY_WORDS).collect();
    let last = words.len() - 1;
    let parts: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let quoted = word.replace('"', "\"\"");
            if i == last {
                format!("\"{quoted}\"*")
            } else {
                format!("\"{quoted}\"")
            }
        })
        .collect();
    Ok(Some(parts.join(" ")))
}

/// `GET /api/articles/{id}`: ein eigener Artikel mit Inhalt.
pub async fn show(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<ArticleFull>, ApiError> {
    let article = sqlx::query_as::<_, FullRow>(
        "SELECT a.id, a.url, a.title, a.excerpt, a.content, a.is_read, a.created_at, \
                (SELECT json_group_array(name) FROM ( \
                    SELECT t.name FROM article_tags at JOIN tags t ON t.id = at.tag_id \
                    WHERE at.article_id = a.id ORDER BY t.name)) AS tags \
         FROM articles a WHERE a.id = ? AND a.user_id = ?",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    article.map(|row| Json(row.into())).ok_or_else(not_found)
}

/// `PATCH /api/articles/{id}`: Gelesen-Status und/oder Schlagwörter ändern.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    ApiJson(patch): ApiJson<ArticlePatch>,
) -> Result<Json<ArticleSummary>, ApiError> {
    let new_tags = patch.tags.as_deref().map(tags::normalize).transpose()?;

    let mut tx = state.pool.begin().await?;
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT id FROM articles WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?;
    if owned.is_none() {
        return Err(not_found());
    }
    if let Some(is_read) = patch.is_read {
        sqlx::query("UPDATE articles SET is_read = ? WHERE id = ?")
            .bind(is_read)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(names) = new_tags {
        tags::replace(&mut tx, user.id, id, &names).await?;
    }
    let article = load_summary(&mut tx, user.id, id).await?;
    tx.commit().await?;
    Ok(Json(article))
}

async fn load_summary(
    conn: &mut SqliteConnection,
    user_id: i64,
    id: i64,
) -> Result<ArticleSummary, ApiError> {
    let row = sqlx::query_as::<_, SummaryRow>(
        "SELECT a.id, a.url, a.title, a.excerpt, NULL AS snippet, a.is_read, a.created_at, \
                (SELECT json_group_array(name) FROM ( \
                    SELECT t.name FROM article_tags at JOIN tags t ON t.id = at.tag_id \
                    WHERE at.article_id = a.id ORDER BY t.name)) AS tags \
         FROM articles a WHERE a.id = ? AND a.user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(conn)
    .await?;
    row.map(Into::into).ok_or_else(not_found)
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
