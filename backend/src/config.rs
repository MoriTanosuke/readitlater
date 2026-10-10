//! Konfiguration ausschließlich über Umgebungsvariablen.

use std::{env, str::FromStr};

use crate::ratelimit::RateLimitConfig;

#[derive(Debug, Clone)]
pub struct Config {
    /// Pfad zur SQLite-Datei (`DATABASE_PATH`).
    pub database_path: String,
    /// Adresse, auf der das Backend lauscht (`BIND_ADDR`).
    pub bind_addr: String,
    /// Darf sich jemand neu registrieren? (`REGISTRATION_ENABLED`)
    pub registration_enabled: bool,
    /// Session-Cookie nur über HTTPS senden (`COOKIE_SECURE`).
    pub cookie_secure: bool,
    /// Lebensdauer einer Session in Tagen (`SESSION_DAYS`).
    pub session_days: i64,
    /// Abruf von Adressen im eigenen Netz erlauben (`FETCH_ALLOW_PRIVATE`).
    /// ACHTUNG: Hebt den SSRF-Schutz auf. Nur für Tests oder ein vertrauenswürdiges,
    /// isoliertes Netz verwenden. Standard: aus.
    pub fetch_allow_private: bool,
    /// Gesamt-Timeout für das Laden einer Seite in Sekunden (`FETCH_TIMEOUT_SECS`).
    pub fetch_timeout_secs: u64,
    /// Maximale Größe einer geladenen Seite in Bytes (`FETCH_MAX_BYTES`).
    pub fetch_max_bytes: usize,
    /// Maximal gleichzeitige Seitenabrufe (`FETCH_CONCURRENCY`).
    pub fetch_concurrency: usize,
    /// Ratenbegrenzung (`RATE_LIMIT_ENABLED`, Standard: an). Die Werte der einzelnen
    /// Stufen stehen in `ratelimit.rs`.
    pub rate_limits: RateLimitConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_path: "./data/app.db".into(),
            bind_addr: "0.0.0.0:3000".into(),
            registration_enabled: true,
            cookie_secure: false,
            session_days: 30,
            fetch_allow_private: false,
            fetch_timeout_secs: 15,
            fetch_max_bytes: 2 * 1024 * 1024,
            fetch_concurrency: 4,
            rate_limits: RateLimitConfig::default(),
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            database_path: env::var("DATABASE_PATH").unwrap_or(defaults.database_path),
            bind_addr: env::var("BIND_ADDR").unwrap_or(defaults.bind_addr),
            registration_enabled: env_bool("REGISTRATION_ENABLED", defaults.registration_enabled),
            cookie_secure: env_bool("COOKIE_SECURE", defaults.cookie_secure),
            session_days: env_positive("SESSION_DAYS", defaults.session_days),
            fetch_allow_private: env_bool("FETCH_ALLOW_PRIVATE", defaults.fetch_allow_private),
            fetch_timeout_secs: env_positive("FETCH_TIMEOUT_SECS", defaults.fetch_timeout_secs),
            fetch_max_bytes: env_positive("FETCH_MAX_BYTES", defaults.fetch_max_bytes),
            fetch_concurrency: env_positive("FETCH_CONCURRENCY", defaults.fetch_concurrency),
            rate_limits: RateLimitConfig {
                enabled: env_bool("RATE_LIMIT_ENABLED", defaults.rate_limits.enabled),
                ..defaults.rate_limits
            },
        }
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

/// Liest eine Zahl größer als 0; bei fehlender oder ungültiger Angabe gilt der Standardwert.
fn env_positive<T>(name: &str, default: T) -> T
where
    T: FromStr + PartialOrd + Default,
{
    env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<T>().ok())
        .filter(|v| *v > T::default())
        .unwrap_or(default)
}
