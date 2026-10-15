//! Befehle für den Betreiber, die auf dem Server im Container laufen
//! (`docker compose exec app /app/readlater-backend reset-credentials <E-Mail>`).
//!
//! `reset-credentials` ist der Rückweg, wenn sich ein Benutzer ausgesperrt hat (Passwort
//! vergessen und kein Passkey mehr). Es gibt bewusst keinen Reset per E-Mail: Die App verschickt
//! keine Mails, und ein Postfach als Generalschlüssel würde auch die Phishing-Resistenz der
//! Passkeys aushebeln. Wer Zugriff auf den Container hat, darf das.

use sqlx::SqlitePool;

use crate::auth;

#[derive(Debug)]
pub struct ResetOptions {
    pub email: String,
    pub new_password: String,
    /// Auch alle Passkeys des Benutzers löschen (bei Verdacht auf Missbrauch).
    pub revoke_passkeys: bool,
    /// Auch alle Tokens zum Teilen aus Apps löschen.
    pub revoke_share_tokens: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResetReport {
    pub sessions_ended: u64,
    pub passkeys_removed: u64,
    pub share_tokens_removed: u64,
}

#[derive(Debug)]
pub enum ResetError {
    UnknownUser,
    InvalidPassword(String),
    Database(String),
}

impl std::fmt::Display for ResetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownUser => write!(f, "Es gibt kein Konto mit dieser E-Mail-Adresse"),
            Self::InvalidPassword(message) => write!(f, "{message}"),
            Self::Database(message) => write!(f, "Datenbankfehler: {message}"),
        }
    }
}

impl std::error::Error for ResetError {}

impl From<sqlx::Error> for ResetError {
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err.to_string())
    }
}

/// Setzt das Passwort neu und beendet alle Sitzungen des Benutzers, in einer Transaktion.
/// Es gelten dieselben Regeln für das Passwort und dasselbe Hash-Verfahren wie in der App.
/// Ohne weitere Angaben bleiben Passkeys und Tokens erhalten.
pub async fn reset_credentials(
    pool: &SqlitePool,
    options: &ResetOptions,
) -> Result<ResetReport, ResetError> {
    auth::validate_password(&options.new_password)
        .map_err(|_| ResetError::InvalidPassword(password_rules()))?;
    let hash = auth::hash_password(&options.new_password)
        .map_err(|_| ResetError::Database("Passwort konnte nicht verarbeitet werden".into()))?;

    let mut tx = pool.begin().await?;
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
        .bind(options.email.trim().to_lowercase())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ResetError::UnknownUser)?;

    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let sessions_ended = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let passkeys_removed = if options.revoke_passkeys {
        sqlx::query("DELETE FROM passkeys WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    } else {
        0
    };
    let share_tokens_removed = if options.revoke_share_tokens {
        sqlx::query("DELETE FROM share_tokens WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    } else {
        0
    };
    tx.commit().await?;
    Ok(ResetReport {
        sessions_ended,
        passkeys_removed,
        share_tokens_removed,
    })
}

fn password_rules() -> String {
    "Das Passwort muss zwischen 10 und 256 Zeichen lang sein".into()
}

/// Ergebnis des Einlesens der Befehlszeile.
#[derive(Debug, PartialEq, Eq)]
pub struct ResetArgs {
    pub email: String,
    pub revoke_passkeys: bool,
    pub revoke_share_tokens: bool,
    pub password_stdin: bool,
}

pub const RESET_USAGE: &str = "Aufruf: readlater-backend reset-credentials <E-Mail> \
[--revoke-passkeys] [--revoke-share-tokens] [--password-stdin]\n\
\n\
  --revoke-passkeys       auch alle Passkeys des Benutzers löschen\n\
  --revoke-share-tokens   auch alle Tokens zum Teilen aus Apps löschen\n\
  --password-stdin        neues Passwort aus der Standardeingabe lesen statt nachzufragen";

/// Liest die Argumente nach `reset-credentials`.
pub fn parse_reset_args(args: &[String]) -> Result<ResetArgs, String> {
    let mut email = None;
    let mut parsed = ResetArgs {
        email: String::new(),
        revoke_passkeys: false,
        revoke_share_tokens: false,
        password_stdin: false,
    };
    for arg in args {
        match arg.as_str() {
            "--revoke-passkeys" => parsed.revoke_passkeys = true,
            "--revoke-share-tokens" => parsed.revoke_share_tokens = true,
            "--password-stdin" => parsed.password_stdin = true,
            flag if flag.starts_with("--") => return Err(format!("Unbekannte Option: {flag}")),
            value if email.is_none() => email = Some(value.to_owned()),
            _ => return Err("Es darf nur eine E-Mail-Adresse angegeben werden".into()),
        }
    }
    parsed.email = email.ok_or("Die E-Mail-Adresse fehlt")?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_email_and_flags_in_any_order() {
        let parsed = parse_reset_args(&args(&[
            "--revoke-passkeys",
            "a@example.org",
            "--password-stdin",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            ResetArgs {
                email: "a@example.org".into(),
                revoke_passkeys: true,
                revoke_share_tokens: false,
                password_stdin: true,
            }
        );
    }

    #[test]
    fn rejects_missing_email_unknown_flags_and_extra_arguments() {
        assert!(parse_reset_args(&args(&[])).is_err());
        assert!(parse_reset_args(&args(&["--revoke-passkeys"])).is_err());
        assert!(parse_reset_args(&args(&["a@example.org", "--password=geheim"])).is_err());
        assert!(parse_reset_args(&args(&["a@example.org", "b@example.org"])).is_err());
    }
}
