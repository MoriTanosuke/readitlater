//! Schlagwörter: Eingaben prüfen, einem Artikel zuordnen und auflisten.

use axum::{Json, extract::State};
use serde::Serialize;
use sqlx::SqliteConnection;

use crate::{AppState, auth::AuthUser, error::ApiError};

const MAX_TAG_LEN: usize = 40;
const MAX_TAGS_PER_ARTICLE: usize = 20;

#[derive(Serialize, sqlx::FromRow)]
pub struct TagCount {
    name: String,
    count: i64,
}

/// `GET /api/tags`: eigene Schlagwörter mit Anzahl der Artikel, alphabetisch.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TagCount>>, ApiError> {
    let tags = sqlx::query_as::<_, TagCount>(
        "SELECT t.name AS name, COUNT(at.article_id) AS count \
         FROM tags t JOIN article_tags at ON at.tag_id = t.id \
         WHERE t.user_id = ? GROUP BY t.id ORDER BY t.name",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(tags))
}

/// Bereinigt eine Liste von Schlagwörtern: Leerraum wird zusammengefasst,
/// leere Einträge entfallen, Doppelte (ohne Beachtung der Groß-/Kleinschreibung)
/// werden zusammengelegt.
pub fn normalize(raw: &[String]) -> Result<Vec<String>, ApiError> {
    let mut result: Vec<String> = Vec::new();
    for name in raw {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            continue;
        }
        if name.chars().count() > MAX_TAG_LEN {
            return Err(ApiError::BadRequest(format!(
                "Ein Schlagwort darf höchstens {MAX_TAG_LEN} Zeichen lang sein"
            )));
        }
        if name.contains(',') || name.chars().any(char::is_control) {
            return Err(ApiError::BadRequest(
                "Schlagwörter dürfen weder Kommas noch Steuerzeichen enthalten".into(),
            ));
        }
        if !result
            .iter()
            .any(|t| t.to_lowercase() == name.to_lowercase())
        {
            result.push(name);
        }
    }
    if result.len() > MAX_TAGS_PER_ARTICLE {
        return Err(ApiError::BadRequest(format!(
            "Ein Artikel darf höchstens {MAX_TAGS_PER_ARTICLE} Schlagwörter haben"
        )));
    }
    Ok(result)
}

/// Ersetzt die Schlagwörter eines Artikels (der Artikel gehört dem Benutzer,
/// das prüft der Aufrufer) und räumt nicht mehr verwendete Schlagwörter auf.
pub async fn replace(
    conn: &mut SqliteConnection,
    user_id: i64,
    article_id: i64,
    names: &[String],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM article_tags WHERE article_id = ?")
        .bind(article_id)
        .execute(&mut *conn)
        .await?;
    for name in names {
        // Die Spalte name ist NOCASE: „Rust“ und „rust“ sind dasselbe Schlagwort.
        sqlx::query("INSERT INTO tags (user_id, name) VALUES (?, ?) ON CONFLICT DO NOTHING")
            .bind(user_id)
            .bind(name)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO article_tags (article_id, tag_id) \
             SELECT ?, id FROM tags WHERE user_id = ? AND name = ?",
        )
        .bind(article_id)
        .bind(user_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "DELETE FROM tags WHERE user_id = ? \
         AND id NOT IN (SELECT tag_id FROM article_tags)",
    )
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn leerraum_und_doppelte_werden_bereinigt() {
        let result =
            normalize(&names(&["  Rust ", "rust", "", "Web  Entwicklung", "RUST"])).unwrap();
        assert_eq!(result, ["Rust", "Web Entwicklung"]);
    }

    #[test]
    fn ungueltige_schlagwoerter_werden_abgelehnt() {
        assert!(normalize(&names(&["a,b"])).is_err());
        assert!(
            normalize(&names(&["a\nb"])).is_ok(),
            "Zeilenumbruch ist Leerraum"
        );
        assert!(normalize(&names(&["a\u{7}b"])).is_err());
        assert!(normalize(&names(&[&"x".repeat(41)])).is_err());
        let many: Vec<String> = (0..21).map(|i| format!("t{i}")).collect();
        assert!(normalize(&many).is_err());
    }
}
