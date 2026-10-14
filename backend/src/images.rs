//! Bilder der Artikel lokal ablegen: Warteschlange, Hintergrund-Worker und Auslieferung.
//!
//! Beim Speichern eines Artikels trägt `enqueue` jedes Bild (`<img src>` im bereinigten HTML)
//! als Zeile in `article_images` ein (Status `pending`) und weckt den Worker. Der Worker
//! (`run_worker`, in `main` gestartet) holt jedes Bild mit dem SSRF-geschützten `Fetcher`,
//! verkleinert es in `imageproc` und speichert das Ergebnis als BLOB (`done`). Misslingt es,
//! bleibt das Bild beim Original-Link: Der Artikel selbst ändert sich nie.
//!
//! Die Warteschlange liegt in der Datenbank. Ein Neustart verliert nichts: `recover` macht
//! unterbrochene Aufträge (`working`) wieder zu `pending`, ein Minutentakt holt fällige
//! Wiederholungen. Es ist weder ein Cronjob noch ein zweiter Dienst nötig.
//!
//! Wiederholt wird nur bei vorübergehenden Fehlern (Zeitüberschreitung, Verbindungsfehler,
//! 5xx, 429), höchstens dreimal mit wachsendem Abstand. Alles andere (gesperrte Adresse,
//! 404, kein Bild, zu groß) ist endgültig (`failed`).

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::task::JoinSet;
use url::Url;

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    extract,
    fetch::FetchError,
    imageproc::{self, ProcessError},
};

/// Mehr Bilder je Artikel werden nicht lokal abgelegt (die übrigen bleiben Original-Links).
pub const MAX_IMAGES_PER_ARTICLE: usize = 30;
const MAX_ATTEMPTS: i64 = 3;
/// Wartezeit vor dem nächsten Versuch: 60 s nach dem ersten, 300 s nach dem zweiten.
const RETRY_BASE_SECS: i64 = 60;
/// So oft prüft der Worker auch ohne Weckruf, ob etwas fällig ist.
const IDLE_CHECK: Duration = Duration::from_secs(60);

/// Ein Bild, das lokal vorliegt: Adresse für den `src` und Maße (vermeidet Layoutsprünge).
#[derive(Serialize, Debug)]
pub struct ImageRef {
    pub src: String,
    pub width: i64,
    pub height: i64,
}

/// Lokale Bilder eines Artikels, Schlüssel ist die Original-Adresse (absolut).
pub async fn for_article(
    pool: &SqlitePool,
    article_id: i64,
) -> Result<BTreeMap<String, ImageRef>, sqlx::Error> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id, url, width, height FROM article_images \
         WHERE article_id = ? AND status = 'done' ORDER BY id",
    )
    .bind(article_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, url, width, height)| {
            (
                url,
                ImageRef {
                    src: format!("/api/articles/{article_id}/images/{id}"),
                    width,
                    height,
                },
            )
        })
        .collect())
}

/// Trägt die Bilder des Artikels in die Warteschlange ein (bereits bekannte bleiben, wie sie
/// sind) und weckt den Worker. Gibt die Zahl neu eingetragener Bilder zurück.
pub async fn enqueue(
    state: &AppState,
    article_id: i64,
    content_html: &str,
    base: &Url,
) -> Result<u64, sqlx::Error> {
    let urls = extract::image_urls(content_html, base, MAX_IMAGES_PER_ARTICLE);
    let mut added = 0;
    for url in &urls {
        added +=
            sqlx::query("INSERT OR IGNORE INTO article_images (article_id, url) VALUES (?, ?)")
                .bind(article_id)
                .bind(url.as_str())
                .execute(&state.pool)
                .await?
                .rows_affected();
    }
    if added > 0 {
        state.image_wakeup.notify_one();
    }
    Ok(added)
}

/// `POST /api/articles/{id}/refresh-images`: Bilder eines (älteren oder importierten)
/// Artikels einreihen und endgültig gescheiterte noch einmal versuchen.
pub async fn refresh(
    State(state): State<AppState>,
    user: AuthUser,
    Path(article_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !state.config.image_store_enabled {
        return Err(ApiError::Conflict(
            "Das lokale Speichern von Bildern ist abgeschaltet".into(),
        ));
    }
    let article: Option<(String, String)> =
        sqlx::query_as("SELECT url, content FROM articles WHERE id = ? AND user_id = ?")
            .bind(article_id)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((url, content)) = article else {
        return Err(ApiError::NotFound("Artikel nicht gefunden".into()));
    };
    let base = Url::parse(&url).map_err(ApiError::internal)?;

    // Zählt wie ein Abruf: Das Nachladen soll das Kontingent nicht umgehen.
    state.limits.check_fetch(user.id)?;
    enqueue(&state, article_id, &content, &base).await?;
    sqlx::query(
        "UPDATE article_images SET status = 'pending', attempts = 0, next_try_at = 0 \
         WHERE article_id = ? AND status = 'failed'",
    )
    .bind(article_id)
    .execute(&state.pool)
    .await?;
    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM article_images \
         WHERE article_id = ? AND status IN ('pending', 'working')",
    )
    .bind(article_id)
    .fetch_one(&state.pool)
    .await?;
    state.image_wakeup.notify_one();
    Ok(Json(serde_json::json!({ "queued": queued })))
}

/// `GET /api/articles/{id}/images/{image_id}`: ein lokal abgelegtes Bild des eigenen Artikels.
pub async fn show(
    State(state): State<AppState>,
    user: AuthUser,
    Path((article_id, image_id)): Path<(i64, i64)>,
) -> Result<Response, ApiError> {
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT i.mime, i.data FROM article_images i \
         JOIN articles a ON a.id = i.article_id \
         WHERE i.id = ? AND a.id = ? AND a.user_id = ? AND i.status = 'done'",
    )
    .bind(image_id)
    .bind(article_id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((mime, data)) = row else {
        return Err(ApiError::NotFound("Bild nicht gefunden".into()));
    };
    let mut response = Response::new(Body::from(data));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    // Der Typ stammt aus unserer eigenen Verarbeitung, nie vom fremden Server.
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(ApiError::internal)?,
    );
    // Der Inhalt einer Bild-Id ändert sich nie (Nachladen betrifft nur nicht fertige Bilder).
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// Beim Start: Aufträge, die ein Neustart unterbrochen hat, wieder einreihen.
pub async fn recover(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE article_images SET status = 'pending' WHERE status = 'working'")
        .execute(pool)
        .await?;
    Ok(())
}

/// Dauerhafte Hintergrundaufgabe: arbeitet fällige Bilder ab und wartet dann auf einen
/// Weckruf oder den Minutentakt.
pub async fn run_worker(state: AppState) {
    if let Err(e) = recover(&state.pool).await {
        tracing::error!("Bilder-Warteschlange konnte nicht wiederhergestellt werden: {e}");
    }
    loop {
        process_pending(&state).await;
        tokio::select! {
            () = state.image_wakeup.notified() => {}
            () = tokio::time::sleep(IDLE_CHECK) => {}
        }
    }
}

/// Arbeitet alle jetzt fälligen Aufträge ab (höchstens `image_concurrency` gleichzeitig) und
/// gibt die Zahl bearbeiteter Aufträge zurück. Auch für Tests, die nicht warten wollen.
pub async fn process_pending(state: &AppState) -> usize {
    let mut tasks = JoinSet::new();
    for _ in 0..state.config.image_concurrency.max(1) {
        let state = state.clone();
        tasks.spawn(async move {
            let mut handled = 0;
            loop {
                match claim(&state.pool).await {
                    Ok(Some(job)) => {
                        process_job(&state, job).await;
                        handled += 1;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("Bilder-Warteschlange: {e}");
                        break;
                    }
                }
            }
            handled
        });
    }
    let mut total = 0;
    while let Some(result) = tasks.join_next().await {
        total += result.unwrap_or(0);
    }
    total
}

#[derive(sqlx::FromRow)]
struct Job {
    id: i64,
    url: String,
    attempts: i64,
    article_url: String,
}

/// Nimmt den nächsten fälligen Auftrag in einer Anweisung an sich (kein Doppelzugriff).
async fn claim(pool: &SqlitePool) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "UPDATE article_images SET status = 'working', attempts = attempts + 1 \
         WHERE id = (SELECT id FROM article_images \
                     WHERE status = 'pending' AND next_try_at <= unixepoch() \
                     ORDER BY id LIMIT 1) \
         RETURNING id, url, attempts, \
                   (SELECT url FROM articles WHERE id = article_images.article_id) AS article_url",
    )
    .fetch_optional(pool)
    .await
}

enum Failure {
    /// Kann beim nächsten Mal klappen.
    Retry(String),
    /// Wird nie klappen.
    Permanent(String),
}

async fn process_job(state: &AppState, job: Job) {
    match download_and_process(state, &job).await {
        Ok(done) => {
            let saved = sqlx::query(
                "UPDATE article_images \
                 SET status = 'done', mime = ?, width = ?, height = ?, data = ? \
                 WHERE id = ? AND status = 'working'",
            )
            .bind(done.mime)
            .bind(i64::from(done.width))
            .bind(i64::from(done.height))
            .bind(done.data)
            .bind(job.id)
            .execute(&state.pool)
            .await;
            // Artikel inzwischen gelöscht: die Zeile ist weg, nichts zu tun.
            if let Err(e) = saved {
                tracing::warn!("Bild {} konnte nicht gespeichert werden: {e}", job.id);
            }
        }
        Err(Failure::Retry(reason)) if job.attempts < MAX_ATTEMPTS => {
            tracing::debug!("Bild {} später erneut ({reason})", job.url);
            let delay = RETRY_BASE_SECS * 5_i64.pow((job.attempts - 1).clamp(0, 4) as u32);
            let _ = sqlx::query(
                "UPDATE article_images \
                 SET status = 'pending', next_try_at = unixepoch() + ? WHERE id = ?",
            )
            .bind(delay)
            .bind(job.id)
            .execute(&state.pool)
            .await;
        }
        Err(Failure::Retry(reason) | Failure::Permanent(reason)) => {
            tracing::debug!("Bild {} bleibt beim Original ({reason})", job.url);
            let _ = sqlx::query(
                "UPDATE article_images SET status = 'failed', data = NULL WHERE id = ?",
            )
            .bind(job.id)
            .execute(&state.pool)
            .await;
        }
    }
}

async fn download_and_process(
    state: &AppState,
    job: &Job,
) -> Result<imageproc::Processed, Failure> {
    let url = Url::parse(&job.url).map_err(|_| Failure::Permanent("ungültige Adresse".into()))?;
    let mut referer =
        Url::parse(&job.article_url).map_err(|_| Failure::Permanent("Artikel fehlt".into()))?;
    referer.set_fragment(None);

    let bytes = state
        .fetcher
        .fetch_image(&url, &referer, state.config.image_max_bytes)
        .await
        .map_err(|e| match e {
            FetchError::Timeout | FetchError::Upstream(_) => Failure::Retry(format!("{e:?}")),
            FetchError::Status(status)
                if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS =>
            {
                Failure::Retry(format!("{e:?}"))
            }
            other => Failure::Permanent(format!("{other:?}")),
        })?;

    // Dekodieren und Verkleinern rechnen lange: nicht auf einem Async-Thread.
    tokio::task::spawn_blocking(move || imageproc::process(&bytes))
        .await
        .map_err(|e| Failure::Permanent(e.to_string()))?
        .map_err(|e: ProcessError| Failure::Permanent(format!("{e:?}")))
}
