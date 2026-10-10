//! Ratenbegrenzung in Stufen. Der Zustand liegt nur im Speicher (ein Prozess, ein Neustart
//! setzt die Zähler zurück). In SQLite zu zählen würde jede Anfrage zu einem Schreibzugriff
//! machen und den einzigen Schreiber der Datenbank zum Engpass.
//!
//! | Stufe | Schlüssel | Wo geprüft |
//! |---|---|---|
//! | `general` | Client-IP | Middleware für alle Routen außer `/health` |
//! | `auth` | Client-IP | Middleware für Login, Registrierung, Passwort ändern, Konto löschen |
//! | `login_pair` | IP und E-Mail | `auth::login` |
//! | `login_email` | E-Mail | `auth::login` (Obergrenze gegen verteilte Angriffe) |
//! | `password_user` | Benutzer | Passwort ändern, Konto löschen |
//! | `fetch_user` | Benutzer | `articles::save_article` (Speichern und Teilen) |
//!
//! Die Prüfung ist ein Hash-Zugriff und kostet Nanosekunden. Sie steht immer vor dem
//! Parsen des Bodys, der Authentifizierung und dem Hashing. Die Zahl der E-Mail-Schlüssel
//! ist dadurch begrenzt, dass die IP-Stufen vorher greifen. Ein Hintergrundjob entfernt
//! Einträge, deren Zähler sich wieder erholt haben.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    time::Duration,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{
    Quota, RateLimiter,
    clock::{Clock, DefaultClock},
    state::keyed::DefaultKeyedStateStore,
};

use crate::{AppState, error::ApiError};

type Keyed<K> = RateLimiter<K, DefaultKeyedStateStore<K>, DefaultClock>;

/// Eine Regel: bis zu `burst` Anfragen sofort, danach eine pro `period_ms` Millisekunden.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub burst: u32,
    pub period_ms: u64,
}

impl Rule {
    pub const fn new(burst: u32, period_secs: u64) -> Self {
        Self {
            burst,
            period_ms: period_secs * 1000,
        }
    }

    pub const fn millis(burst: u32, period_ms: u64) -> Self {
        Self { burst, period_ms }
    }

    fn quota(self) -> Quota {
        Quota::with_period(Duration::from_millis(self.period_ms.max(1)))
            .expect("Zeitraum ist größer als 0")
            .allow_burst(NonZeroU32::new(self.burst.max(1)).expect("Burst ist größer als 0"))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub general: Rule,
    pub auth: Rule,
    pub login_pair: Rule,
    pub login_email: Rule,
    pub password_user: Rule,
    pub fetch_user: Rule,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // 120 Anfragen sofort, danach 2 pro Sekunde.
            general: Rule::millis(120, 500),
            // 10 Versuche sofort, danach einer alle 6 Sekunden.
            auth: Rule::new(10, 6),
            // Gleiches Konto von derselben IP: 5 Versuche, danach einer pro Minute.
            login_pair: Rule::new(5, 60),
            // Gleiches Konto von allen IPs zusammen: 30 Versuche, danach einer alle 2 Minuten.
            login_email: Rule::new(30, 120),
            // Passwortprüfung für angemeldete Aktionen: 5, danach einer pro Minute.
            password_user: Rule::new(5, 60),
            // Seitenabrufe pro Benutzer: 10 sofort, danach einer alle 6 Sekunden.
            fetch_user: Rule::new(10, 6),
        }
    }
}

/// Adresse des Clients (IPv6 auf das /64-Präfix gekürzt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientIp(pub IpAddr);

pub struct Limits {
    enabled: bool,
    general: Keyed<IpAddr>,
    auth: Keyed<IpAddr>,
    login_pair: Keyed<(IpAddr, String)>,
    login_email: Keyed<String>,
    password_user: Keyed<i64>,
    fetch_user: Keyed<i64>,
}

impl Limits {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            enabled: config.enabled,
            general: RateLimiter::keyed(config.general.quota()),
            auth: RateLimiter::keyed(config.auth.quota()),
            login_pair: RateLimiter::keyed(config.login_pair.quota()),
            login_email: RateLimiter::keyed(config.login_email.quota()),
            password_user: RateLimiter::keyed(config.password_user.quota()),
            fetch_user: RateLimiter::keyed(config.fetch_user.quota()),
        }
    }

    fn check<K>(&self, limiter: &Keyed<K>, key: &K) -> Result<(), ApiError>
    where
        K: Clone + Eq + std::hash::Hash,
    {
        if !self.enabled {
            return Ok(());
        }
        limiter.check_key(key).map_err(|not_until| {
            let wait = not_until.wait_time_from(limiter.clock().now());
            ApiError::RateLimited {
                retry_after_secs: wait.as_secs() + 1,
            }
        })
    }

    /// Login: erst das Paar aus IP und E-Mail, dann die E-Mail allein.
    pub fn check_login(&self, ip: ClientIp, email: &str) -> Result<(), ApiError> {
        self.check(&self.login_pair, &(ip.0, email.to_owned()))?;
        self.check(&self.login_email, &email.to_owned())
    }

    pub fn check_password_action(&self, user_id: i64) -> Result<(), ApiError> {
        self.check(&self.password_user, &user_id)
    }

    pub fn check_fetch(&self, user_id: i64) -> Result<(), ApiError> {
        self.check(&self.fetch_user, &user_id)
    }

    /// Entfernt Einträge, deren Zähler sich erholt haben.
    pub fn purge(&self) {
        self.general.retain_recent();
        self.general.shrink_to_fit();
        self.auth.retain_recent();
        self.auth.shrink_to_fit();
        self.login_pair.retain_recent();
        self.login_pair.shrink_to_fit();
        self.login_email.retain_recent();
        self.login_email.shrink_to_fit();
        self.password_user.retain_recent();
        self.password_user.shrink_to_fit();
        self.fetch_user.retain_recent();
        self.fetch_user.shrink_to_fit();
    }
}

/// Ermittelt die Client-Adresse. Das Backend ist nur über Caddy erreichbar, und Caddy
/// setzt `X-Forwarded-For` auf die echte Adresse. Wir nehmen den letzten Eintrag, also den,
/// den der nächste Proxy angehängt hat. Vom Client mitgeschickte Werte stehen davor und
/// werden ignoriert. Ohne Header gilt die Adresse der direkten Verbindung.
pub fn client_ip(request: &Request) -> ClientIp {
    let forwarded = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .rfind(|s| !s.is_empty())
        .and_then(|s| s.parse::<IpAddr>().ok());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    ClientIp(normalize(
        forwarded
            .or(peer)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
    ))
}

/// IPv6-Adressen eines Anschlusses teilen sich ein /64-Netz, deshalb zählt das Präfix.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(octets.into())
            }
        },
    }
}

/// Äußerste Stufe: bestimmt die Client-Adresse, legt sie für die Handler ab und
/// wendet das allgemeine Limit an (außer für `/health`).
pub async fn general(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let ip = client_ip(&request);
    request.extensions_mut().insert(ip);
    if !request.uri().path().ends_with("/health")
        && let Err(e) = state.limits.check(&state.limits.general, &ip.0)
    {
        return e.into_response();
    }
    next.run(request).await
}

/// Strengere Stufe für Routen, die Passwörter prüfen oder hashen.
pub async fn auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let ip = request
        .extensions()
        .get::<ClientIp>()
        .copied()
        .unwrap_or_else(|| client_ip(&request));
    if let Err(e) = state.limits.check(&state.limits.auth, &ip.0) {
        return e.into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request(forwarded: &[&str]) -> Request {
        let mut builder = Request::builder().uri("/api/x");
        for value in forwarded {
            builder = builder.header("x-forwarded-for", *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn last_forwarded_entry_wins() {
        assert_eq!(
            client_ip(&request(&["6.6.6.6, 203.0.113.7"])).0,
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip(&request(&["6.6.6.6", "203.0.113.8"])).0,
            "203.0.113.8".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn garbage_header_falls_back_to_unspecified() {
        assert_eq!(
            client_ip(&request(&["kein-ip"])).0,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(
            client_ip(&request(&[])).0,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn ipv6_is_reduced_to_its_64_prefix() {
        let a = normalize("2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap());
        let b = normalize("2001:db8:1:2::1".parse().unwrap());
        let c = normalize("2001:db8:1:3::1".parse().unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            normalize("::ffff:192.0.2.1".parse().unwrap()),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
    }
}
