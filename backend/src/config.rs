//! Konfiguration ausschließlich über Umgebungsvariablen.

use std::env;

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
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_path: env::var("DATABASE_PATH").unwrap_or_else(|_| "./data/app.db".into()),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into()),
            registration_enabled: env_bool("REGISTRATION_ENABLED", true),
            cookie_secure: env_bool("COOKIE_SECURE", false),
            session_days: env::var("SESSION_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|d| *d > 0)
                .unwrap_or(30),
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
