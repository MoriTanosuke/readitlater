//! Hauptinhalt einer Webseite extrahieren (Readability) und das HTML bereinigen.

use dom_smoothie::{Config, Readability, ReadabilityError};
use url::Url;

/// Obergrenze für Elemente im Dokument; schützt vor extrem aufgeblähten Seiten.
const MAX_ELEMENTS: usize = 30_000;
/// Weniger lesbarer Text gilt als „kein Artikel“.
const MIN_TEXT_CHARS: usize = 40;
const MAX_TITLE_CHARS: usize = 300;
const MAX_EXCERPT_CHARS: usize = 300;

#[derive(Debug)]
pub struct Extracted {
    pub title: String,
    /// Bereinigtes HTML (nur harmlose Tags und Attribute).
    pub content_html: String,
    /// Reiner Text mit normalisierten Leerzeichen (Grundlage der Suche).
    pub content_text: String,
    pub excerpt: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// Kein lesbarer Artikeltext gefunden.
    NoContent,
    /// Dokument ist zu groß oder zu verschachtelt.
    TooComplex,
}

pub fn extract(html: &str, url: &Url) -> Result<Extracted, ExtractError> {
    let config = Config {
        max_elements_to_parse: MAX_ELEMENTS,
        ..Config::default()
    };
    let mut readability = Readability::new(html, Some(url.as_str()), Some(config))
        .map_err(|_| ExtractError::NoContent)?;
    let article = readability.parse().map_err(|e| match e {
        ReadabilityError::TooManyElements(..) => ExtractError::TooComplex,
        _ => ExtractError::NoContent,
    })?;

    let content_text = collapse_whitespace(&article.text_content);
    if content_text.chars().count() < MIN_TEXT_CHARS {
        return Err(ExtractError::NoContent);
    }

    let title = {
        let t = collapse_whitespace(&article.title);
        if t.is_empty() {
            url.host_str().unwrap_or("Ohne Titel").to_owned()
        } else {
            truncate_chars(&t, MAX_TITLE_CHARS)
        }
    };

    let excerpt = article
        .excerpt
        .as_deref()
        .map(collapse_whitespace)
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| content_text.clone());

    Ok(Extracted {
        title,
        content_html: sanitize(&article.content),
        excerpt: truncate_chars(&excerpt, MAX_EXCERPT_CHARS),
        content_text,
    })
}

/// Entfernt alles außer harmlosen Tags und Attributen (Skripte, Event-Handler,
/// `javascript:`-Links, Formulare, Inline-Styles usw.). Links bekommen
/// `rel="noopener noreferrer nofollow"`.
pub fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(html)
        .to_string()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(title: &str, body: &str) -> String {
        format!(
            "<!doctype html><html><head><title>{title}</title></head><body>\
             <nav><a href=\"/\">Start</a></nav><article>{body}</article>\
             <footer>Impressum</footer></body></html>"
        )
    }

    fn paragraphs() -> String {
        "<p>Dies ist ein ausführlicher Absatz mit genug Text, damit die Extraktion ihn sicher \
         als Hauptinhalt der Seite erkennt und nicht als Beiwerk verwirft.</p>"
            .repeat(6)
    }

    fn base() -> Url {
        Url::parse("https://example.com/artikel").unwrap()
    }

    #[test]
    fn extrahiert_titel_und_text() {
        let html = page("Mein Artikel", &paragraphs());
        let out = extract(&html, &base()).unwrap();
        assert_eq!(out.title, "Mein Artikel");
        assert!(out.content_text.contains("ausführlicher Absatz"));
        assert!(!out.content_text.contains("Impressum"));
        assert!(out.excerpt.chars().count() <= MAX_EXCERPT_CHARS);
        assert!(out.content_html.contains("<p>"));
    }

    #[test]
    fn gefaehrliche_inhalte_werden_entfernt() {
        let body = format!(
            "{}<script>alert(1)</script>\
             <p onclick=\"steal()\">Klick <a href=\"javascript:alert(2)\">hier</a> oder \
             <a href=\"https://example.org/\">dort</a>.</p>\
             <img src=\"https://example.org/a.png\" onerror=\"steal()\">\
             <iframe src=\"https://evil.example/\"></iframe>\
             <form action=\"https://evil.example/\"><input name=\"x\"></form>\
             <p style=\"position:fixed\">Stil</p>",
            paragraphs()
        );
        let out = extract(&page("Test", &body), &base()).unwrap();
        let html = out.content_html.to_lowercase();
        for verboten in [
            "<script",
            "onclick",
            "onerror",
            "javascript:",
            "<iframe",
            "<form",
            "<input",
            "style=",
        ] {
            assert!(!html.contains(verboten), "{verboten} in {html}");
        }
        assert!(html.contains("https://example.org/"));
        assert!(html.contains("noopener noreferrer nofollow"));
    }

    #[test]
    fn seite_ohne_artikeltext_wird_abgelehnt() {
        let html = page("Leer", "<p>Kurz.</p>");
        assert_eq!(
            extract(&html, &base()).unwrap_err(),
            ExtractError::NoContent
        );
        assert_eq!(extract("", &base()).unwrap_err(), ExtractError::NoContent);
    }

    #[test]
    fn titel_faellt_auf_den_host_zurueck() {
        let html = page("", &paragraphs());
        let out = extract(&html, &base()).unwrap();
        assert!(!out.title.is_empty());
    }

    #[test]
    fn kuerzen_beachtet_zeichen_statt_bytes() {
        assert_eq!(truncate_chars("äöü", 5), "äöü");
        assert_eq!(truncate_chars("äöüäöü", 4), "äöü…");
    }
}
