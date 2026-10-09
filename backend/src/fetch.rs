//! Abruf fremder Webseiten mit Schutz vor SSRF (Server-Side Request Forgery).
//!
//! Der Server lädt auf Wunsch der Benutzer beliebige URLs. Ohne Schutz könnte man
//! darüber Dienste im Heimnetz oder Metadaten-Endpunkte ansprechen. Die Absicherung
//! besteht aus mehreren Schichten:
//!
//! 1. `validate_url`: nur http/https, keine Zugangsdaten in der URL, IP-Adressen in der
//!    URL müssen öffentlich sein (auch Schreibweisen wie `2130706433` oder `0x7f.1`,
//!    die der URL-Parser zu einer IP normalisiert).
//! 2. `PublicOnlyResolver`: Hostnamen werden beim Verbindungsaufbau aufgelöst und
//!    nur öffentliche Adressen zugelassen. Weil genau diese Adressen auch benutzt
//!    werden, greift der Schutz bei DNS-Rebinding und nach Weiterleitungen.
//! 3. Weiterleitungen werden einzeln geprüft (Ziel-URL wie bei 1., maximal 5).
//! 4. Kein Proxy (sonst würde die Auflösung beim Proxy statt bei uns passieren),
//!    Gesamt-Timeout, Größenlimit für den Inhalt, nur HTML.

use std::{
    error::Error,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    Client, StatusCode,
    dns::{Addrs, Name, Resolve, Resolving},
    header, redirect,
};
use url::{Host, Url};

const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = "Leseliste/0.1 (+https://github.com/MoriTanosuke/readitlater)";

#[derive(Debug)]
pub enum FetchError {
    /// Keine gültige http(s)-Adresse.
    InvalidUrl,
    /// Adresse zeigt auf ein nicht öffentliches Ziel.
    Blocked,
    Timeout,
    TooLarge,
    NotHtml,
    Status(StatusCode),
    TooManyRedirects,
    Upstream(String),
}

/// Fehler, die unsere eigenen Schutzmaßnahmen auslösen (wird in der Fehlerkette gesucht).
#[derive(Debug)]
enum GuardError {
    Blocked,
    TooManyRedirects,
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked => write!(f, "Adresse nicht erlaubt"),
            Self::TooManyRedirects => write!(f, "zu viele Weiterleitungen"),
        }
    }
}

impl Error for GuardError {}

/// Ergebnis eines erfolgreichen Abrufs.
pub struct FetchedPage {
    /// Endgültige URL nach Weiterleitungen (Basis für relative Links).
    pub final_url: Url,
    pub html: String,
}

pub struct Fetcher {
    client: Client,
    max_bytes: usize,
    allow_private: bool,
}

impl Fetcher {
    pub fn new(
        allow_private: bool,
        timeout: Duration,
        max_bytes: usize,
    ) -> Result<Self, reqwest::Error> {
        // reqwest ist ohne festen Krypto-Provider eingebunden (ring, rein statisch baubar).
        // Fehler heißt nur, dass bereits einer installiert ist.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let policy = redirect::Policy::custom(move |attempt| {
            match redirect_decision(attempt.url(), attempt.previous().len(), allow_private) {
                Ok(()) => attempt.follow(),
                Err(guard) => attempt.error(guard),
            }
        });

        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(5))
            .redirect(policy)
            .referer(false)
            .no_proxy();
        if !allow_private {
            builder = builder.dns_resolver(Arc::new(PublicOnlyResolver));
        }
        Ok(Self {
            client: builder.build()?,
            max_bytes,
            allow_private,
        })
    }

    /// Prüft eine URL nach den Regeln dieses Fetchers (ohne Netzwerkzugriff).
    pub fn validate(&self, url: &Url) -> Result<(), FetchError> {
        validate_url(url, self.allow_private)
    }

    pub async fn fetch(&self, url: &Url) -> Result<FetchedPage, FetchError> {
        self.validate(url)?;

        let mut response = self
            .client
            .get(url.clone())
            .header(header::ACCEPT, "text/html,application/xhtml+xml;q=0.9")
            .header(header::ACCEPT_LANGUAGE, "de,en;q=0.8")
            .send()
            .await
            .map_err(classify)?;

        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status));
        }

        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !mime.is_empty() && mime != "text/html" && mime != "application/xhtml+xml" {
            return Err(FetchError::NotHtml);
        }

        if response
            .content_length()
            .is_some_and(|len| len > self.max_bytes as u64)
        {
            return Err(FetchError::TooLarge);
        }

        // Inhalt in Stücken lesen und bei Überschreitung abbrechen. Das Limit gilt für die
        // entpackten Daten, schützt also auch vor Kompressionsbomben.
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(classify)? {
            if body.len() + chunk.len() > self.max_bytes {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }

        Ok(FetchedPage {
            final_url: response.url().clone(),
            html: decode(&body, &content_type),
        })
    }
}

/// Ordnet einen reqwest-Fehler ein und sucht dabei nach unseren Schutz-Fehlern.
fn classify(err: reqwest::Error) -> FetchError {
    let mut source: Option<&(dyn Error + 'static)> = Some(&err);
    while let Some(current) = source {
        if let Some(guard) = current.downcast_ref::<GuardError>() {
            return match guard {
                GuardError::Blocked => FetchError::Blocked,
                GuardError::TooManyRedirects => FetchError::TooManyRedirects,
            };
        }
        source = current.source();
    }
    if err.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Upstream(err.to_string())
    }
}

/// Entscheidung für eine einzelne Weiterleitung (eigene Funktion, damit sie testbar ist).
fn redirect_decision(
    target: &Url,
    hops_so_far: usize,
    allow_private: bool,
) -> Result<(), GuardError> {
    if hops_so_far >= MAX_REDIRECTS {
        return Err(GuardError::TooManyRedirects);
    }
    validate_url(target, allow_private).map_err(|_| GuardError::Blocked)
}

/// Prüft Schema, Zugangsdaten und (bei IP-Adressen) das Ziel. Hostnamen prüft der Resolver.
pub fn validate_url(url: &Url, allow_private: bool) -> Result<(), FetchError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::InvalidUrl);
    }
    match url.host() {
        None => Err(FetchError::InvalidUrl),
        Some(Host::Ipv4(ip)) if !allow_private && !is_public_ip(IpAddr::V4(ip)) => {
            Err(FetchError::Blocked)
        }
        Some(Host::Ipv6(ip)) if !allow_private && !is_public_ip(IpAddr::V6(ip)) => {
            Err(FetchError::Blocked)
        }
        Some(_) => Ok(()),
    }
}

/// DNS-Auflösung, die nur öffentliche Adressen durchlässt.
struct PublicOnlyResolver;

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public: Vec<SocketAddr> = resolved.filter(|a| is_public_ip(a.ip())).collect();
            if public.is_empty() {
                return Err(Box::new(GuardError::Blocked) as Box<dyn Error + Send + Sync>);
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// Ist die Adresse im öffentlichen Internet routbar? Im Zweifel `false`.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local() // 169.254.0.0/16, enthält Cloud-Metadaten
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0 // 0.0.0.0/8
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 (CGNAT)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 (Benchmarks)
        || o[0] >= 240) // 240.0.0.0/4 (reserviert)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // Nur globale Unicast-Adressen (2000::/3). Alles andere (Loopback, fc00::/7,
    // fe80::/10, Multicast, NAT64 usw.) ist gesperrt.
    if (s[0] & 0xe000) != 0x2000 {
        return false;
    }
    let documentation = s[0] == 0x2001 && s[1] == 0x0db8;
    let documentation_new = s[0] == 0x3fff && s[1] < 0x1000; // 3fff::/20
    let teredo = s[0] == 0x2001 && s[1] == 0x0000;
    let six_to_four = s[0] == 0x2002; // enthält eine eingebettete IPv4-Adresse
    !(documentation || documentation_new || teredo || six_to_four)
}

/// Dekodiert den Inhalt: Zeichensatz aus dem Header, sonst aus dem Meta-Tag, sonst UTF-8.
fn decode(body: &[u8], content_type: &str) -> String {
    let label = charset_from_content_type(content_type).or_else(|| charset_from_meta(body));
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(body);
    text.into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_owned())
    })
}

fn charset_from_meta(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(2048)]).to_ascii_lowercase();
    let start = head.find("charset=")? + "charset=".len();
    let rest = head[start..].trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')))
        .unwrap_or(rest.len());
    let label = &rest[..end];
    (!label.is_empty()).then(|| label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn private_und_reservierte_ipv4_sind_gesperrt() {
        for s in [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.178.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "192.0.0.8",
            "198.18.0.1",
            "198.19.255.255",
            "192.0.2.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!is_public_ip(ip(s)), "{s} muss gesperrt sein");
        }
    }

    #[test]
    fn oeffentliche_ipv4_sind_erlaubt() {
        for s in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
        ] {
            assert!(is_public_ip(ip(s)), "{s} muss erlaubt sein");
        }
    }

    #[test]
    fn nicht_oeffentliche_ipv6_sind_gesperrt() {
        for s in [
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "ff02::1",
            "2001:db8::1",
            "2001::1",
            "2002:7f00:1::1",
            "64:ff9b::7f00:1",
            "fec0::1",
            "::ffff:127.0.0.1",
            "::ffff:10.1.2.3",
            "::ffff:169.254.169.254",
        ] {
            assert!(!is_public_ip(ip(s)), "{s} muss gesperrt sein");
        }
    }

    #[test]
    fn oeffentliche_ipv6_sind_erlaubt() {
        for s in [
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(ip(s)), "{s} muss erlaubt sein");
        }
    }

    #[test]
    fn url_pruefung_blockiert_ip_schreibweisen() {
        for s in [
            "http://127.0.0.1/",
            "http://127.1/",
            "http://2130706433/", // dezimal
            "http://0x7f000001/", // hexadezimal
            "http://0177.0.0.1/", // oktal
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5:8080/",
            "https://192.168.1.1/",
        ] {
            assert!(
                matches!(validate_url(&url(s), false), Err(FetchError::Blocked)),
                "{s} muss blockiert werden"
            );
        }
    }

    #[test]
    fn url_pruefung_lehnt_falsche_schemas_und_zugangsdaten_ab() {
        for s in [
            "ftp://example.com/",
            "file:///etc/passwd",
            "gopher://example.com/",
            "http://user@example.com/",
            "http://user:pw@example.com/",
        ] {
            assert!(
                matches!(validate_url(&url(s), false), Err(FetchError::InvalidUrl)),
                "{s} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn url_pruefung_laesst_normale_adressen_durch() {
        for s in [
            "https://example.com/artikel",
            "http://example.org:8080/x?y=1",
            "https://8.8.8.8/",
            "https://[2606:4700:4700::1111]/",
        ] {
            assert!(validate_url(&url(s), false).is_ok(), "{s}");
        }
    }

    #[test]
    fn mit_freigabe_sind_private_ziele_erlaubt() {
        assert!(validate_url(&url("http://127.0.0.1:3000/"), true).is_ok());
        // Schema und Zugangsdaten bleiben auch dann verboten.
        assert!(validate_url(&url("file:///etc/passwd"), true).is_err());
    }

    #[test]
    fn weiterleitungen_werden_geprueft_und_begrenzt() {
        let privat = url("http://192.168.0.1/admin");
        assert!(matches!(
            redirect_decision(&privat, 0, false),
            Err(GuardError::Blocked)
        ));
        let metadaten = url("http://169.254.169.254/");
        assert!(matches!(
            redirect_decision(&metadaten, 1, false),
            Err(GuardError::Blocked)
        ));
        let oeffentlich = url("https://example.com/neu");
        assert!(redirect_decision(&oeffentlich, 4, false).is_ok());
        assert!(matches!(
            redirect_decision(&oeffentlich, MAX_REDIRECTS, false),
            Err(GuardError::TooManyRedirects)
        ));
    }

    #[test]
    fn zeichensatz_aus_header_und_meta() {
        assert_eq!(
            charset_from_content_type("text/html; charset=\"ISO-8859-1\"").as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(charset_from_content_type("text/html"), None);
        assert_eq!(
            charset_from_meta(b"<html><head><meta charset=\"Windows-1252\">").as_deref(),
            Some("windows-1252")
        );
        assert_eq!(
            charset_from_meta(
                b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">"
            )
            .as_deref(),
            Some("utf-8")
        );
        assert_eq!(charset_from_meta(b"<html></html>"), None);
    }

    #[test]
    fn dekodierung_beachtet_den_zeichensatz() {
        assert_eq!(decode(b"K\xe4se", "text/html; charset=iso-8859-1"), "Käse");
        assert_eq!(decode("Käse".as_bytes(), "text/html"), "Käse");
        assert_eq!(
            decode(b"<meta charset=latin1>K\xe4se", "text/html"),
            "<meta charset=latin1>Käse"
        );
    }
}
