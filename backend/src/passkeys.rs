//! Anmeldung mit Passkeys (WebAuthn) als Ergänzung zum Passwort.
//!
//! - Ein Benutzer kann mehrere Passkeys einrichten (höchstens [`MAX_PASSKEYS_PER_USER`]),
//!   zum Beispiel je einen für iCloud-Schlüsselbund, Google-Passwortmanager und einen
//!   Hardware-Schlüssel. Passkeys synchronisieren nicht zwischen Apple und Android.
//! - Das Passwort bleibt Pflicht und Rückfallweg. Es gibt keine Konten nur mit Passkey.
//! - Angemeldet wird ohne E-Mail (discoverable credentials): Der Passkey im Gerät liefert die
//!   Benutzerkennung (`user_handle`, eine zufällige UUID) mit. Deshalb verlangt die
//!   Registrierung einen auffindbaren Passkey (`residentKey: required`).
//! - Hinzufügen und Löschen verlangen das Passwort erneut. Wer nur eine offene Sitzung hat,
//!   kann sich so keinen dauerhaften Zugang einrichten.
//! - Die Zwischenstände der beiden Abläufe (Challenge) liegen nur im Speicher, sind
//!   einmal verwendbar und verfallen nach wenigen Minuten. Ein Neustart bricht angefangene
//!   Abläufe ab, der Benutzer beginnt dann neu.
//! - Die Prüfung der Herkunft (Origin) und der Relying-Party-ID kommt allein aus der
//!   Konfiguration (`PASSKEY_ORIGIN`), nie aus der Anfrage. Ohne Konfiguration sind Passkeys aus.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webauthn_rs::prelude::{
    CredentialID, DiscoverableAuthentication, DiscoverableKey, Passkey, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder,
};

use crate::{
    AppState,
    auth::{self, AuthUser, UserOut},
    error::{ApiError, ApiJson},
};

pub const MAX_PASSKEYS_PER_USER: i64 = 10;
const MAX_NAME_LEN: usize = 60;
/// So lange darf ein angefangener Ablauf dauern (der Browser gibt nach 5 Minuten auf).
const PENDING_TTL: Duration = Duration::from_secs(6 * 60);
/// Obergrenzen gegen unbegrenztes Wachsen des Speichers (Login-Start ist ohne Anmeldung möglich).
const MAX_PENDING_LOGINS: usize = 1000;
const MAX_PENDING_REGISTRATIONS: usize = 100;

// ---------------------------------------------------------------------------
// Dienst: WebAuthn-Konfiguration und Zwischenstände
// ---------------------------------------------------------------------------

pub struct PasskeyService {
    webauthn: Webauthn,
    pending: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    /// Pro Benutzer höchstens eine angefangene Registrierung.
    registrations: HashMap<i64, Timed<PendingRegistration>>,
    /// Angefangene Anmeldungen, Schlüssel ist die Kennung, die der Client zurückschickt.
    logins: HashMap<String, Timed<DiscoverableAuthentication>>,
}

struct Timed<T> {
    created: Instant,
    value: T,
}

struct PendingRegistration {
    name: String,
    user_handle: Uuid,
    state: PasskeyRegistration,
}

impl PasskeyService {
    /// Baut den Dienst aus `PASSKEY_ORIGIN` und optional `PASSKEY_RP_ID`.
    pub fn new(origin: &str, rp_id: Option<&str>) -> Result<Self, String> {
        let url = Url::parse(origin)
            .map_err(|e| format!("PASSKEY_ORIGIN ist keine gültige Adresse: {e}"))?;
        let host = url
            .host_str()
            .ok_or("PASSKEY_ORIGIN braucht einen Hostnamen")?
            .to_owned();
        if url.scheme() != "https" && host != "localhost" {
            return Err(
                "PASSKEY_ORIGIN muss mit https:// beginnen (nur localhost darf http nutzen)".into(),
            );
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err("PASSKEY_ORIGIN darf keinen Pfad enthalten, nur Schema und Host".into());
        }
        let webauthn = WebauthnBuilder::new(rp_id.unwrap_or(&host), &url)
            .map_err(|e| format!("Passkey-Konfiguration ungültig: {e}"))?
            .rp_name("Leseliste")
            .build()
            .map_err(|e| format!("Passkey-Konfiguration ungültig: {e}"))?;
        Ok(Self {
            webauthn,
            pending: Mutex::new(Pending::default()),
        })
    }

    fn pending(&self) -> MutexGuard<'_, Pending> {
        // Eine vergiftete Sperre heißt nur, dass ein anderer Handler abgestürzt ist: Die
        // Zwischenstände bleiben brauchbar.
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Pending {
    fn purge(&mut self) {
        self.registrations
            .retain(|_, p| p.created.elapsed() < PENDING_TTL);
        self.logins.retain(|_, p| p.created.elapsed() < PENDING_TTL);
    }

    fn put_registration(
        &mut self,
        user_id: i64,
        value: PendingRegistration,
    ) -> Result<(), ApiError> {
        if !self.registrations.contains_key(&user_id)
            && self.registrations.len() >= MAX_PENDING_REGISTRATIONS
        {
            self.purge();
            if self.registrations.len() >= MAX_PENDING_REGISTRATIONS {
                return Err(ApiError::TooManyRequests);
            }
        }
        self.registrations.insert(
            user_id,
            Timed {
                created: Instant::now(),
                value,
            },
        );
        Ok(())
    }

    fn take_registration(&mut self, user_id: i64) -> Option<PendingRegistration> {
        let pending = self.registrations.remove(&user_id)?;
        (pending.created.elapsed() < PENDING_TTL).then_some(pending.value)
    }

    fn put_login(&mut self, id: String, value: DiscoverableAuthentication) -> Result<(), ApiError> {
        if self.logins.len() >= MAX_PENDING_LOGINS {
            self.purge();
            if self.logins.len() >= MAX_PENDING_LOGINS {
                return Err(ApiError::TooManyRequests);
            }
        }
        self.logins.insert(
            id,
            Timed {
                created: Instant::now(),
                value,
            },
        );
        Ok(())
    }

    fn take_login(&mut self, id: &str) -> Option<DiscoverableAuthentication> {
        let pending = self.logins.remove(id)?;
        (pending.created.elapsed() < PENDING_TTL).then_some(pending.value)
    }
}

fn service(state: &AppState) -> Result<&Arc<PasskeyService>, ApiError> {
    state
        .passkeys
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Passkeys sind nicht eingerichtet".into()))
}

// ---------------------------------------------------------------------------
// Ausgaben und Eingaben
// ---------------------------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
pub struct PasskeyInfo {
    id: i64,
    name: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterStart {
    name: String,
    /// Bestätigung durch das Passwort.
    password: String,
}

#[derive(Deserialize)]
pub struct RegisterFinish {
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
pub struct LoginFinish {
    id: String,
    credential: PublicKeyCredential,
}

#[derive(Deserialize)]
pub struct Rename {
    name: String,
}

#[derive(Deserialize)]
pub struct PasswordConfirmation {
    password: String,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /api/features`: Ohne Anmeldung abrufbar, damit die Anmeldeseite weiß, ob sie den
/// Knopf „Mit Passkey anmelden“ zeigen soll.
pub async fn features(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "passkeys": state.passkeys.is_some() }))
}

/// `GET /api/passkeys`: eigene Passkeys (ohne Schlüsselmaterial).
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PasskeyInfo>>, ApiError> {
    let passkeys = sqlx::query_as::<_, PasskeyInfo>(
        "SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ? ORDER BY id",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(passkeys))
}

/// `POST /api/passkeys/register/start`: Passwort prüfen und die Optionen für
/// `navigator.credentials.create()` liefern.
pub async fn register_start(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<RegisterStart>,
) -> Result<Json<Value>, ApiError> {
    let service = service(&state)?;
    let name = clean_name(&body.name)?;
    auth::confirm_password(&state, user.id, body.password).await?;

    let existing: Vec<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT credential_id, user_handle FROM passkeys WHERE user_id = ?")
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?;
    if existing.len() as i64 >= MAX_PASSKEYS_PER_USER {
        return Err(too_many_passkeys());
    }
    // Alle Passkeys eines Benutzers teilen eine Kennung. Beim ersten wird sie neu gewürfelt.
    let user_handle = existing
        .iter()
        .find_map(|(_, handle)| Uuid::from_slice(handle).ok())
        .unwrap_or_else(Uuid::new_v4);
    // Bereits eingerichtete Passkeys ausschließen: Der Browser lehnt dann ab, wenn dieses Gerät
    // oder dieser Passwortmanager schon einen hat.
    let exclude: Vec<CredentialID> = existing
        .into_iter()
        .map(|(credential_id, _)| credential_id)
        .collect();

    let (creation, registration) = service
        .webauthn
        .start_passkey_registration(
            user_handle,
            &user.email,
            &user.email,
            (!exclude.is_empty()).then_some(exclude),
        )
        .map_err(ApiError::internal)?;
    let mut options = serde_json::to_value(&creation).map_err(ApiError::internal)?;
    require_resident_key(&mut options);

    service.pending().put_registration(
        user.id,
        PendingRegistration {
            name,
            user_handle,
            state: registration,
        },
    )?;
    Ok(Json(options))
}

/// `POST /api/passkeys/register/finish`: Antwort des Geräts prüfen und den Passkey speichern.
pub async fn register_finish(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<RegisterFinish>,
) -> Result<(StatusCode, Json<PasskeyInfo>), ApiError> {
    let service = service(&state)?;
    let pending = service
        .pending()
        .take_registration(user.id)
        .ok_or_else(|| {
            ApiError::BadRequest(
                "Die Einrichtung ist abgelaufen. Bitte noch einmal beginnen".into(),
            )
        })?;
    let passkey = service
        .webauthn
        .finish_passkey_registration(&body.credential, &pending.state)
        .map_err(|e| {
            tracing::info!(
                "Passkey-Registrierung für Benutzer {} abgelehnt: {e}",
                user.id
            );
            ApiError::BadRequest("Der Passkey konnte nicht geprüft werden".into())
        })?;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM passkeys WHERE user_id = ?")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;
    if count >= MAX_PASSKEYS_PER_USER {
        return Err(too_many_passkeys());
    }

    let serialized = serde_json::to_string(&passkey).map_err(ApiError::internal)?;
    let credential_id = passkey.cred_id().clone();
    let inserted = sqlx::query_as::<_, PasskeyInfo>(
        "INSERT INTO passkeys (user_id, name, credential_id, user_handle, passkey) \
         VALUES (?, ?, ?, ?, ?) \
         RETURNING id, name, created_at, last_used_at",
    )
    .bind(user.id)
    .bind(&pending.name)
    .bind(credential_id)
    .bind(pending.user_handle.as_bytes().to_vec())
    .bind(serialized)
    .fetch_one(&state.pool)
    .await;
    match inserted {
        Ok(info) => {
            tracing::info!("Passkey {} für Benutzer {} angelegt", info.id, user.id);
            Ok((StatusCode::CREATED, Json(info)))
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(ApiError::Conflict(
            "Dieser Passkey ist bereits eingerichtet".into(),
        )),
        Err(e) => Err(e.into()),
    }
}

/// `PATCH /api/passkeys/{id}`: umbenennen.
pub async fn rename(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<Rename>,
) -> Result<Json<PasskeyInfo>, ApiError> {
    let name = clean_name(&body.name)?;
    sqlx::query_as::<_, PasskeyInfo>(
        "UPDATE passkeys SET name = ? WHERE id = ? AND user_id = ? \
         RETURNING id, name, created_at, last_used_at",
    )
    .bind(name)
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or_else(|| ApiError::NotFound("Passkey nicht gefunden".into()))
}

/// `DELETE /api/passkeys/{id}`: entfernen, nach Bestätigung mit dem Passwort. Der Passkey im
/// Gerät oder Passwortmanager bleibt bestehen und muss dort separat gelöscht werden.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<PasswordConfirmation>,
) -> Result<StatusCode, ApiError> {
    auth::confirm_password(&state, user.id, body.password).await?;
    let result = sqlx::query("DELETE FROM passkeys WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Passkey nicht gefunden".into()));
    }
    tracing::info!("Passkey {id} von Benutzer {} gelöscht", user.id);
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/passkeys/login/start`: Optionen für `navigator.credentials.get()` ohne
/// Vorgabe eines Kontos (der Passkey im Gerät nennt es selbst).
pub async fn login_start(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let service = service(&state)?;
    let (request, authentication) = service
        .webauthn
        .start_discoverable_authentication()
        .map_err(ApiError::internal)?;
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    let id = hex::encode(bytes);
    service.pending().put_login(id.clone(), authentication)?;
    let options = serde_json::to_value(&request).map_err(ApiError::internal)?;
    Ok(Json(json!({ "id": id, "options": options })))
}

/// `POST /api/passkeys/login/finish`: Signatur prüfen, Zähler nachführen, Sitzung anlegen.
pub async fn login_finish(
    State(state): State<AppState>,
    jar: CookieJar,
    ApiJson(body): ApiJson<LoginFinish>,
) -> Result<(CookieJar, Json<UserOut>), ApiError> {
    let service = service(&state)?;
    // Der Zwischenstand ist nur einmal verwendbar (gegen Wiederholung abgefangener Antworten).
    let authentication = service
        .pending()
        .take_login(&body.id)
        .ok_or(ApiError::BadPasskey)?;
    let (user_handle, _) = service
        .webauthn
        .identify_discoverable_authentication(&body.credential)
        .map_err(|_| ApiError::BadPasskey)?;

    let rows: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, user_id, passkey FROM passkeys WHERE user_handle = ?")
            .bind(user_handle.as_bytes().to_vec())
            .fetch_all(&state.pool)
            .await?;
    let mut entries: Vec<StoredPasskey> = Vec::new();
    for (row_id, user_id, serialized) in rows {
        match serde_json::from_str::<Passkey>(&serialized) {
            Ok(passkey) => entries.push(StoredPasskey {
                row_id,
                user_id,
                passkey,
            }),
            Err(e) => tracing::warn!("Passkey {row_id} lässt sich nicht lesen: {e}"),
        }
    }
    if entries.is_empty() {
        return Err(ApiError::BadPasskey);
    }
    let candidates: Vec<DiscoverableKey> = entries
        .iter()
        .map(|entry| DiscoverableKey::from(&entry.passkey))
        .collect();
    let result = service
        .webauthn
        .finish_discoverable_authentication(&body.credential, authentication, &candidates)
        .map_err(|e| {
            tracing::info!("Anmeldung mit Passkey abgelehnt: {e}");
            ApiError::BadPasskey
        })?;

    let entry = entries
        .iter_mut()
        .find(|entry| entry.passkey.cred_id() == result.cred_id())
        .ok_or(ApiError::BadPasskey)?;
    // Den Zähler nachführen, damit ein geklonter Passkey auffällt.
    let changed = entry.passkey.update_credential(&result) == Some(true);
    if changed {
        let serialized = serde_json::to_string(&entry.passkey).map_err(ApiError::internal)?;
        sqlx::query(
            "UPDATE passkeys SET passkey = ?, \
             last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
        )
        .bind(serialized)
        .bind(entry.row_id)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE passkeys SET last_used_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
        )
        .bind(entry.row_id)
        .execute(&state.pool)
        .await?;
    }

    let (id, email): (i64, String) = sqlx::query_as("SELECT id, email FROM users WHERE id = ?")
        .bind(entry.user_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::BadPasskey)?;
    auth::purge_expired_sessions(&state).await;
    let token = auth::create_session(&state, id).await?;
    let jar = jar.add(auth::session_cookie(&state, token));
    Ok((jar, Json(UserOut { id, email })))
}

struct StoredPasskey {
    row_id: i64,
    user_id: i64,
    passkey: Passkey,
}

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

fn too_many_passkeys() -> ApiError {
    ApiError::Unprocessable(format!(
        "Es sind höchstens {MAX_PASSKEYS_PER_USER} Passkeys möglich. Bitte zuerst einen löschen"
    ))
}

/// Bereinigt den Namen eines Passkeys (Leerraum zusammenfassen, 1 bis 60 Zeichen, keine Steuerzeichen).
fn clean_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest(format!(
            "Der Name muss 1 bis {MAX_NAME_LEN} Zeichen lang sein"
        )));
    }
    Ok(name)
}

/// Verlangt in den Optionen für den Browser einen auffindbaren Passkey. webauthn-rs setzt hier
/// „discouraged“, doch die Anmeldung ohne E-Mail braucht einen Passkey, den das Gerät selbst
/// findet. Das ist nur ein Hinweis an den Authenticator, die Prüfung auf dem Server ändert sich
/// dadurch nicht.
fn require_resident_key(options: &mut Value) {
    if let Some(selection) = options
        .pointer_mut("/publicKey/authenticatorSelection")
        .and_then(Value::as_object_mut)
    {
        selection.insert("residentKey".into(), json!("required"));
        selection.insert("requireResidentKey".into(), json!(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_must_be_https_without_path() {
        assert!(PasskeyService::new("https://lesen.example.org", None).is_ok());
        assert!(PasskeyService::new("http://localhost:3000", None).is_ok());
        assert!(PasskeyService::new("http://lesen.example.org", None).is_err());
        assert!(PasskeyService::new("https://lesen.example.org/app", None).is_err());
        assert!(PasskeyService::new("kein url", None).is_err());
    }

    #[test]
    fn rp_id_must_be_a_suffix_of_the_host() {
        assert!(PasskeyService::new("https://lesen.example.org", Some("example.org")).is_ok());
        assert!(PasskeyService::new("https://lesen.example.org", Some("andere.de")).is_err());
    }

    #[test]
    fn names_are_normalized_and_limited() {
        assert_eq!(clean_name("  Mein   iPhone ").unwrap(), "Mein iPhone");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(61)).is_err());
        assert!(clean_name("a\u{7}b").is_err());
    }

    #[test]
    fn resident_key_is_required_in_the_options() {
        let mut options = json!({"publicKey": {"authenticatorSelection": {
            "residentKey": "discouraged", "requireResidentKey": false, "userVerification": "required"
        }}});
        require_resident_key(&mut options);
        let selection = &options["publicKey"]["authenticatorSelection"];
        assert_eq!(selection["residentKey"], "required");
        assert_eq!(selection["requireResidentKey"], true);
        assert_eq!(selection["userVerification"], "required");
    }
}
