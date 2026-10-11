//! Eigene Daten exportieren und importieren.
//!
//! Der Export ist eine einzelne JSON-Datei, die der Browser ohne weitere Werkzeuge
//! speichert (normaler Download, keine Komprimierung). Er enthält ausschließlich die
//! Daten des angemeldeten Benutzers, keine E-Mail-Adresse und keine Zugangsdaten.
//!
//! Der Import nimmt nur dieses Format an und lädt **nichts aus dem Netz**. Weil die Datei
//! von Hand verändert sein kann, wird nichts ungeprüft übernommen: Adressen müssen
//! http oder https sein, das HTML wird erneut bereinigt, Schlagwörter und Größen werden
//! geprüft. Artikel, deren Adresse der Benutzer schon hat, bleiben unverändert.

use std::io;

use axum::{
    Json,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{AppState, articles, auth::AuthUser, error::ApiError, extract, highlights, tags};

pub const FORMAT: &str = "leseliste-export";
/// Aktuelle Version der Datei. Version 2 kam mit den Markierungen hinzu; Version 1 (ohne
/// Markierungen) lässt sich weiterhin einlesen.
pub const VERSION: u32 = 2;
const MIN_VERSION: u32 = 1;
/// Größte erlaubte Importdatei in Bytes.
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMPORT_ARTICLES: usize = 20_000;
/// Obergrenze für Inhalt und Klartext eines einzelnen Artikels in Bytes.
const MAX_ARTICLE_BYTES: usize = 4 * 1024 * 1024;
const MAX_URL_LEN: usize = 2048;
const MAX_TITLE_CHARS: usize = 300;
const MAX_EXCERPT_CHARS: usize = 300;
const EXPORT_BATCH: i64 = 50;

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
struct ExportRow {
    #[serde(skip)]
    id: i64,
    url: String,
    title: String,
    excerpt: String,
    content: String,
    content_text: String,
    is_read: bool,
    created_at: String,
    #[sqlx(rename = "tags")]
    #[serde(rename = "tags", serialize_with = "serialize_tags")]
    tags_json: String,
    #[sqlx(rename = "highlights")]
    #[serde(rename = "highlights", serialize_with = "serialize_highlights")]
    highlights_json: String,
}

/// Schlagwörter kommen aus SQLite als JSON-Array-Text und werden als Array ausgegeben.
fn serialize_tags<S: serde::Serializer>(json: &str, serializer: S) -> Result<S::Ok, S::Error> {
    let tags: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    serializer.collect_seq(tags)
}

/// Markierungen kommen aus SQLite als JSON-Array-Text und werden als Array ausgegeben.
fn serialize_highlights<S: serde::Serializer>(
    json: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let items: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    serializer.collect_seq(items)
}

enum Phase {
    Head(String),
    Rows { last_id: i64, first: bool },
    Done,
}

/// `GET /api/export`: alle eigenen Artikel als JSON-Datei zum Herunterladen.
/// Die Antwort wird Stück für Stück erzeugt, der Speicherbedarf bleibt klein.
pub async fn export(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // Ein Link von einer fremden Seite darf keinen Download anstoßen.
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() == b"cross-site")
    {
        return Err(ApiError::Forbidden("Anfrage von fremder Seite".into()));
    }
    state.limits.check_export(user.id)?;

    let (now, day): (String, String) =
        sqlx::query_as("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%d', 'now')")
            .fetch_one(&state.pool)
            .await?;
    let head = format!(
        "{{\"format\":\"{FORMAT}\",\"version\":{VERSION},\"exported_at\":\"{now}\",\"articles\":["
    );

    let body = stream::unfold(
        (state.pool.clone(), user.id, Phase::Head(head)),
        |(pool, user_id, phase)| async move {
            match phase {
                Phase::Head(head) => Some((
                    Ok(Bytes::from(head)),
                    (
                        pool,
                        user_id,
                        Phase::Rows {
                            last_id: 0,
                            first: true,
                        },
                    ),
                )),
                Phase::Rows { last_id, first } => match export_batch(&pool, user_id, last_id).await
                {
                    Ok(rows) if rows.is_empty() => {
                        Some((Ok(Bytes::from_static(b"]}")), (pool, user_id, Phase::Done)))
                    }
                    Ok(rows) => {
                        let last_id = rows.last().map_or(last_id, |r| r.id);
                        let mut out = Vec::new();
                        for (i, row) in rows.iter().enumerate() {
                            if !(first && i == 0) {
                                out.push(b',');
                            }
                            if let Err(e) = serde_json::to_writer(&mut out, row) {
                                return Some((
                                    Err(io::Error::other(e)),
                                    (pool, user_id, Phase::Done),
                                ));
                            }
                        }
                        Some((
                            Ok(Bytes::from(out)),
                            (
                                pool,
                                user_id,
                                Phase::Rows {
                                    last_id,
                                    first: false,
                                },
                            ),
                        ))
                    }
                    Err(e) => {
                        tracing::error!("Export fehlgeschlagen: {e}");
                        Some((Err(io::Error::other(e)), (pool, user_id, Phase::Done)))
                    }
                },
                Phase::Done => None,
            }
        },
    );

    let mut response = Response::new(Body::from_stream(body));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&format!(
        "attachment; filename=\"leseliste-export-{day}.json\""
    )) {
        h.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

async fn export_batch(
    pool: &SqlitePool,
    user_id: i64,
    after_id: i64,
) -> Result<Vec<ExportRow>, sqlx::Error> {
    sqlx::query_as::<_, ExportRow>(
        "SELECT a.id, a.url, a.title, a.excerpt, a.content, a.content_text, a.is_read, \
                a.created_at, \
                (SELECT json_group_array(name) FROM ( \
                    SELECT t.name FROM article_tags at JOIN tags t ON t.id = at.tag_id \
                    WHERE at.article_id = a.id ORDER BY t.name)) AS tags, \
                (SELECT json_group_array(json_object( \
                    'start', start_offset, 'end', end_offset, 'quote', quote, \
                    'prefix', prefix, 'suffix', suffix, 'color', color, 'note', note, \
                    'created_at', created_at)) FROM ( \
                    SELECT * FROM highlights WHERE article_id = a.id \
                    ORDER BY start_offset, id)) AS highlights \
         FROM articles a WHERE a.user_id = ? AND a.id > ? ORDER BY a.id LIMIT ?",
    )
    .bind(user_id)
    .bind(after_id)
    .bind(EXPORT_BATCH)
    .fetch_all(pool)
    .await
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ImportFile {
    format: String,
    version: u32,
    articles: Vec<ImportArticle>,
}

#[derive(Deserialize)]
struct ImportArticle {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    excerpt: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    content_text: String,
    #[serde(default)]
    is_read: bool,
    created_at: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    /// Einzeln geprüft: ein kaputter Eintrag soll nicht die ganze Datei ungültig machen.
    #[serde(default)]
    highlights: Vec<serde_json::Value>,
}

struct Prepared {
    url: String,
    title: String,
    excerpt: String,
    content: String,
    content_text: String,
    is_read: bool,
    created_at: Option<String>,
    tags: Vec<String>,
    highlights: Vec<highlights::Clean>,
    highlights_invalid: usize,
}

#[derive(Serialize, Debug)]
pub struct ImportResult {
    /// Neu angelegte Artikel.
    imported: usize,
    /// Artikel, deren Adresse schon vorhanden war (unverändert gelassen).
    duplicates: usize,
    /// Einträge, die nicht übernommen werden konnten (z. B. ungültige Adresse).
    invalid: usize,
    /// Markierungen, die mit neu angelegten Artikeln übernommen wurden.
    highlights: usize,
    /// Markierungen, die ungültig waren oder über der Höchstzahl lagen.
    highlights_invalid: usize,
}

fn bad_file() -> ApiError {
    ApiError::BadRequest("Das ist keine Exportdatei dieser App".into())
}

/// `POST /api/import`: Exportdatei dieser App einlesen. Es wird nichts aus dem Netz geladen.
pub async fn import(
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> Result<Json<ImportResult>, ApiError> {
    state.limits.check_import(user.id)?;
    // Immer nur ein Import gleichzeitig: Bereinigen ist CPU-lastig, der Schreibzugriff exklusiv.
    let _slot = state
        .import_slot
        .try_acquire()
        .map_err(|_| ApiError::RateLimited {
            retry_after_secs: 30,
        })?;

    let file: ImportFile = serde_json::from_slice(&body).map_err(|_| bad_file())?;
    drop(body);
    if file.format != FORMAT {
        return Err(bad_file());
    }
    if !(MIN_VERSION..=VERSION).contains(&file.version) {
        return Err(ApiError::BadRequest(format!(
            "Die Version {} der Exportdatei wird nicht unterstützt",
            file.version
        )));
    }
    if file.articles.len() > MAX_IMPORT_ARTICLES {
        return Err(ApiError::BadRequest(format!(
            "Die Datei enthält mehr als {MAX_IMPORT_ARTICLES} Artikel"
        )));
    }

    let total = file.articles.len();
    let prepared = tokio::task::spawn_blocking(move || {
        file.articles
            .into_iter()
            .filter_map(prepare)
            .collect::<Vec<_>>()
    })
    .await
    .map_err(ApiError::internal)?;
    let mut result = ImportResult {
        imported: 0,
        duplicates: 0,
        invalid: total - prepared.len(),
        highlights: 0,
        highlights_invalid: 0,
    };

    let mut tx = state.pool.begin().await?;
    for article in prepared {
        let id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO articles \
                 (user_id, url, title, content, content_text, excerpt, is_read, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, COALESCE(?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))) \
             ON CONFLICT (user_id, url) DO NOTHING RETURNING id",
        )
        .bind(user.id)
        .bind(&article.url)
        .bind(&article.title)
        .bind(&article.content)
        .bind(&article.content_text)
        .bind(&article.excerpt)
        .bind(article.is_read)
        .bind(&article.created_at)
        .fetch_optional(&mut *tx)
        .await?;
        match id {
            Some(id) => {
                tags::attach(&mut tx, user.id, id, &article.tags).await?;
                for h in &article.highlights {
                    sqlx::query(
                        "INSERT INTO highlights (user_id, article_id, start_offset, end_offset, \
                             quote, prefix, suffix, color, note, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, \
                             COALESCE(?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')))",
                    )
                    .bind(user.id)
                    .bind(id)
                    .bind(h.start)
                    .bind(h.end)
                    .bind(&h.quote)
                    .bind(&h.prefix)
                    .bind(&h.suffix)
                    .bind(&h.color)
                    .bind(&h.note)
                    .bind(&h.created_at)
                    .execute(&mut *tx)
                    .await?;
                }
                result.highlights += article.highlights.len();
                result.highlights_invalid += article.highlights_invalid;
                result.imported += 1;
            }
            None => result.duplicates += 1,
        }
    }
    tx.commit().await?;
    tracing::info!(
        "Import für Benutzer {}: {} neu, {} schon vorhanden, {} ungültig",
        user.id,
        result.imported,
        result.duplicates,
        result.invalid
    );
    Ok(Json(result))
}

/// Prüft und bereinigt einen Eintrag der Datei. `None`, wenn er unbrauchbar ist.
fn prepare(raw: ImportArticle) -> Option<Prepared> {
    if raw.url.len() > MAX_URL_LEN
        || raw.content.len() > MAX_ARTICLE_BYTES
        || raw.content_text.len() > MAX_ARTICLE_BYTES
    {
        return None;
    }
    let url = articles::parse_url(&raw.url).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let title = collapse(&raw.title);
    let title = if title.is_empty() {
        url.host_str().unwrap_or("Ohne Titel").to_owned()
    } else {
        truncate(&title, MAX_TITLE_CHARS)
    };
    // Extrem tief verschachteltes HTML macht die Bereinigung quadratisch teuer: ungültig.
    if extract::exceeds_max_depth(&raw.content) {
        return None;
    }
    let raw_count = raw.highlights.len();
    let highlights: Vec<highlights::Clean> = raw
        .highlights
        .into_iter()
        .filter_map(|v| serde_json::from_value::<highlights::HighlightInput>(v).ok())
        .filter_map(|h| highlights::validate(h).ok())
        .take(highlights::MAX_PER_ARTICLE)
        .collect();
    Some(Prepared {
        highlights_invalid: raw_count - highlights.len(),
        highlights,
        url: url.as_str().to_owned(),
        title,
        excerpt: truncate(&collapse(&raw.excerpt), MAX_EXCERPT_CHARS),
        // Die Datei kann verändert worden sein: HTML immer neu bereinigen.
        content: extract::sanitize(&raw.content),
        content_text: collapse(&raw.content_text),
        is_read: raw.is_read,
        created_at: raw.created_at.filter(|t| valid_timestamp(t)),
        tags: tags::normalize_lenient(&raw.tags),
    })
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// Erwartet das Format, das die App selbst schreibt: `2026-10-11T08:30:00Z`.
pub(crate) fn valid_timestamp(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_must_match_the_exported_format() {
        assert!(valid_timestamp("2026-10-11T08:30:00Z"));
        assert!(!valid_timestamp("2026-10-11 08:30:00"));
        assert!(!valid_timestamp("2026-10-11T08:30:00+02:00"));
        assert!(!valid_timestamp("'; DROP TABLE articles;--"));
        assert!(!valid_timestamp(""));
    }

    fn article(url: &str) -> ImportArticle {
        ImportArticle {
            url: url.into(),
            title: "  Ein   Titel ".into(),
            excerpt: String::new(),
            content: "<p>Text</p><script>alert(1)</script>".into(),
            content_text: "Text".into(),
            is_read: true,
            created_at: Some("2026-01-02T03:04:05Z".into()),
            tags: vec!["Rust".into(), "a,b".into(), "rust".into()],
            highlights: Vec::new(),
        }
    }

    #[test]
    fn prepare_cleans_everything_from_the_file() {
        let p = prepare(article("https://example.org/a#teil")).unwrap();
        assert_eq!(p.url, "https://example.org/a");
        assert_eq!(p.title, "Ein Titel");
        assert!(!p.content.contains("script"));
        assert_eq!(p.tags, ["Rust"]);
        assert_eq!(p.created_at.as_deref(), Some("2026-01-02T03:04:05Z"));
    }

    #[test]
    fn prepare_rejects_unusable_addresses() {
        for url in [
            "javascript:alert(1)",
            "ftp://example.org/a",
            "file:///etc/passwd",
            "https://nutzer:geheim@example.org/",
            "",
        ] {
            assert!(prepare(article(url)).is_none(), "{url}");
        }
    }
}
