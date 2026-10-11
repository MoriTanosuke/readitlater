//! Markierungen und Notizen zu Artikeln.
//!
//! Jede Abfrage prüft den Besitzer (`user_id`), fremde Markierungen sind nie sichtbar. Die
//! Prüfung der Eingaben (`validate`) gilt für die API und für den Import gleichermaßen.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    AppState,
    auth::AuthUser,
    error::{ApiError, ApiJson},
};

/// Höchstzahl an Markierungen je Artikel.
pub const MAX_PER_ARTICLE: usize = 500;
/// Größte erlaubte Position (Artikeltexte sind auf wenige MiB begrenzt).
const MAX_OFFSET: i64 = 8_000_000;
const MAX_QUOTE_CHARS: usize = 2000;
const MAX_CONTEXT_CHARS: usize = 100;
const MAX_NOTE_CHARS: usize = 2000;

/// Eingabe für eine neue Markierung (API und Importdatei).
#[derive(Deserialize)]
pub struct HighlightInput {
    pub start: i64,
    pub end: i64,
    pub quote: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    pub color: String,
    #[serde(default)]
    pub note: Option<String>,
    /// Nur beim Import ausgewertet; die API ignoriert den Wert.
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Geprüfte Markierung, bereit zum Speichern.
#[derive(Debug)]
pub struct Clean {
    pub start: i64,
    pub end: i64,
    pub quote: String,
    pub prefix: String,
    pub suffix: String,
    pub color: String,
    pub note: Option<String>,
    pub created_at: Option<String>,
}

fn invalid(message: &str) -> ApiError {
    ApiError::BadRequest(message.to_owned())
}

/// Farbe als `#rrggbb` in Kleinbuchstaben.
pub fn clean_color(raw: &str) -> Result<String, ApiError> {
    let b = raw.as_bytes();
    if b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit) {
        Ok(raw.to_ascii_lowercase())
    } else {
        Err(invalid("Die Farbe muss die Form #rrggbb haben"))
    }
}

/// Notiz bereinigen: nur Text, ohne Steuerzeichen (außer Zeilenumbruch), leer wird `None`.
pub fn clean_note(raw: &str) -> Result<Option<String>, ApiError> {
    let text: String = raw
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();
    let text = text.trim();
    if text.chars().count() > MAX_NOTE_CHARS {
        return Err(invalid("Die Notiz ist zu lang (höchstens 2000 Zeichen)"));
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

pub fn validate(input: HighlightInput) -> Result<Clean, ApiError> {
    if input.start < 0 || input.end <= input.start || input.end > MAX_OFFSET {
        return Err(invalid("Ungültige Position der Markierung"));
    }
    let quote_len = input.quote.chars().count();
    if quote_len == 0 || quote_len > MAX_QUOTE_CHARS || input.quote.trim().is_empty() {
        return Err(invalid("Der markierte Text ist leer oder zu lang"));
    }
    if input.prefix.chars().count() > MAX_CONTEXT_CHARS
        || input.suffix.chars().count() > MAX_CONTEXT_CHARS
    {
        return Err(invalid("Der Textkontext der Markierung ist zu lang"));
    }
    let note = match input.note.as_deref() {
        Some(raw) => clean_note(raw)?,
        None => None,
    };
    Ok(Clean {
        start: input.start,
        end: input.end,
        quote: input.quote,
        prefix: input.prefix,
        suffix: input.suffix,
        color: clean_color(&input.color)?,
        note,
        created_at: input
            .created_at
            .filter(|t| crate::transfer::valid_timestamp(t)),
    })
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Highlight {
    id: i64,
    #[sqlx(rename = "start_offset")]
    #[serde(rename = "start")]
    start_offset: i64,
    #[sqlx(rename = "end_offset")]
    #[serde(rename = "end")]
    end_offset: i64,
    quote: String,
    prefix: String,
    suffix: String,
    color: String,
    note: Option<String>,
    created_at: String,
}

fn not_found() -> ApiError {
    ApiError::NotFound("Markierung nicht gefunden".into())
}

fn article_not_found() -> ApiError {
    ApiError::NotFound("Artikel nicht gefunden".into())
}

/// Alle Markierungen eines eigenen Artikels, nach Position sortiert.
pub async fn for_article(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    article_id: i64,
) -> Result<Vec<Highlight>, sqlx::Error> {
    sqlx::query_as::<_, Highlight>(
        "SELECT id, start_offset, end_offset, quote, prefix, suffix, color, note, created_at \
         FROM highlights WHERE article_id = ? AND user_id = ? ORDER BY start_offset, id",
    )
    .bind(article_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// `GET /api/articles/{id}/highlights`
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(article_id): Path<i64>,
) -> Result<Json<Vec<Highlight>>, ApiError> {
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT id FROM articles WHERE id = ? AND user_id = ?")
            .bind(article_id)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    if owned.is_none() {
        return Err(article_not_found());
    }
    let rows = for_article(&state.pool, user.id, article_id).await?;
    Ok(Json(rows))
}

/// `POST /api/articles/{id}/highlights`
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(article_id): Path<i64>,
    ApiJson(input): ApiJson<HighlightInput>,
) -> Result<(StatusCode, Json<Highlight>), ApiError> {
    let clean = validate(input)?;
    let mut tx = state.pool.begin().await?;
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT id FROM articles WHERE id = ? AND user_id = ?")
            .bind(article_id)
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?;
    if owned.is_none() {
        return Err(article_not_found());
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM highlights WHERE article_id = ?")
        .bind(article_id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_PER_ARTICLE as i64 {
        return Err(ApiError::Conflict(format!(
            "Höchstens {MAX_PER_ARTICLE} Markierungen pro Artikel"
        )));
    }
    let row = sqlx::query_as::<_, Highlight>(
        "INSERT INTO highlights \
             (user_id, article_id, start_offset, end_offset, quote, prefix, suffix, color, note) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id, start_offset, end_offset, quote, prefix, suffix, color, note, created_at",
    )
    .bind(user.id)
    .bind(article_id)
    .bind(clean.start)
    .bind(clean.end)
    .bind(&clean.quote)
    .bind(&clean.prefix)
    .bind(&clean.suffix)
    .bind(&clean.color)
    .bind(&clean.note)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// Unterscheidet „Feld fehlt“ (`None`) von „Feld ist null“ (`Some(None)`).
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
pub struct HighlightPatch {
    color: Option<String>,
    /// `null` löscht die Notiz, fehlt das Feld, bleibt sie unverändert.
    #[serde(default, deserialize_with = "present")]
    note: Option<Option<String>>,
}

/// `PATCH /api/highlights/{id}`: Farbe und/oder Notiz ändern.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    ApiJson(patch): ApiJson<HighlightPatch>,
) -> Result<Json<Highlight>, ApiError> {
    if patch.color.is_none() && patch.note.is_none() {
        return Err(invalid("Nichts zu ändern"));
    }
    let color = patch.color.as_deref().map(clean_color).transpose()?;
    let set_note = patch.note.is_some();
    let note = match patch.note.as_ref().and_then(|n| n.as_deref()) {
        Some(raw) => clean_note(raw)?,
        None => None,
    };
    let row = sqlx::query_as::<_, Highlight>(
        "UPDATE highlights SET color = COALESCE(?, color), \
             note = CASE WHEN ? THEN ? ELSE note END \
         WHERE id = ? AND user_id = ? \
         RETURNING id, start_offset, end_offset, quote, prefix, suffix, color, note, created_at",
    )
    .bind(color)
    .bind(set_note)
    .bind(note)
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    row.map(Json).ok_or_else(not_found)
}

/// `DELETE /api/highlights/{id}`
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("DELETE FROM highlights WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> HighlightInput {
        HighlightInput {
            start: 3,
            end: 9,
            quote: "Ein Satz".into(),
            prefix: String::new(),
            suffix: String::new(),
            color: "#FFEB3B".into(),
            note: Some("  Notiz\r\nzwei \u{7} ".into()),
            created_at: None,
        }
    }

    #[test]
    fn valid_input_is_normalized() {
        let c = validate(input()).unwrap();
        assert_eq!(c.color, "#ffeb3b");
        assert_eq!(c.note.as_deref(), Some("Notiz\nzwei"));
    }

    #[test]
    fn broken_input_is_rejected() {
        for mutate in [
            (|i: &mut HighlightInput| i.start = -1) as fn(&mut HighlightInput),
            |i| i.end = i.start,
            |i| i.end = 9_000_000,
            |i| i.quote = "   ".into(),
            |i| i.quote = "x".repeat(2001),
            |i| i.prefix = "x".repeat(101),
            |i| i.color = "red".into(),
            |i| i.color = "#12345g".into(),
            |i| i.color = "url(x)".into(),
            |i| i.note = Some("n".repeat(2001)),
        ] {
            let mut i = input();
            mutate(&mut i);
            assert!(validate(i).is_err());
        }
    }

    #[test]
    fn empty_note_becomes_none() {
        assert_eq!(clean_note(" \n ").unwrap(), None);
    }
}
