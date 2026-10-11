//! Hauptinhalt einer Webseite extrahieren (Readability) und das HTML bereinigen.

use dom_smoothie::{Config, Readability, ReadabilityError};
use url::Url;

/// Obergrenze für Elemente im Dokument; schützt vor extrem aufgeblähten Seiten.
const MAX_ELEMENTS: usize = 30_000;
/// Weniger lesbarer Text gilt als „kein Artikel“.
/// Größte erlaubte Verschachtelungstiefe. Echte Seiten liegen weit darunter (meist < 60);
/// bei tausenden geschachtelten Elementen braucht die Extraktion Minuten CPU-Zeit
/// (gemessen: 5.000 Ebenen > 2 Minuten, siehe `exceeds_max_depth`) und kann den Stack überlaufen lassen.
const MAX_DEPTH: usize = 256;
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
    /// Sprache des Textes (ISO 639-1), nur wenn die Erkennung sicher ist.
    pub lang: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// Kein lesbarer Artikeltext gefunden.
    NoContent,
    /// Dokument ist zu groß oder zu verschachtelt.
    TooComplex,
}

/// Elemente ohne Ende-Tag oder mit optionalem Ende-Tag: Sie schachteln im HTML-Parser nicht
/// (ein neues `<li>` schließt das alte), zählen also nicht zur Tiefe.
const FLAT_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr", "p", "li", "dt", "dd", "tr", "td", "th", "thead", "tbody", "tfoot", "colgroup",
    "option", "optgroup", "rt", "rp", "param",
];
/// Elemente, deren Inhalt als reiner Text gilt (dort ist `<` kein Tag).
const RAW_TEXT_TAGS: &[&str] = &["script", "style", "textarea", "title"];

/// Prüft vor der Extraktion, ob das Dokument zu tief verschachtelt ist.
///
/// Das Parsen selbst ist bei tausenden Ebenen schon quadratisch teuer (gemessen: 25.000
/// Ebenen brauchen im Debug-Build über 30 s), darum geht das nicht über einen echten Parser.
/// Stattdessen läuft ein einfacher Durchgang in linearer Zeit über die Tags und zählt offene
/// minus geschlossene Elemente. Er ist bewusst grob: Nicht geschlossene Tags (`<span>` ohne
/// Ende) zählen als offen und können eine kaputte Seite ablehnen, was hier gewollt ist.
pub fn exceeds_max_depth(html: &str) -> bool {
    let b = html.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    while let Some(off) = b[i..].iter().position(|&c| c == b'<') {
        i += off + 1;
        let rest = &b[i..];
        if rest.starts_with(b"!--") {
            i += find(&b[i..], b"-->").map_or(b.len() - i, |p| p + 3);
            continue;
        }
        if matches!(rest.first(), Some(b'!' | b'?')) {
            i += find(rest, b">").map_or(rest.len(), |p| p + 1);
            continue;
        }
        let closing = rest.first() == Some(&b'/');
        let name_start = i + usize::from(closing);
        let name_len = b[name_start.min(b.len())..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if name_len == 0 {
            continue; // „<“ als Text
        }
        let name = html[name_start..name_start + name_len].to_ascii_lowercase();
        i = name_start + name_len;
        // Ende des Tags suchen, `>` in Anführungszeichen überspringen.
        let mut quote = 0u8;
        while i < b.len() {
            let c = b[i];
            i += 1;
            match (quote, c) {
                (0, b'>') => break,
                (0, b'"' | b'\'') => quote = c,
                (q, c) if q != 0 && q == c => quote = 0,
                _ => {}
            }
        }
        if FLAT_TAGS.contains(&name.as_str()) {
            continue;
        }
        if closing {
            depth = depth.saturating_sub(1);
            continue;
        }
        depth += 1;
        if depth > MAX_DEPTH {
            return true;
        }
        if RAW_TEXT_TAGS.contains(&name.as_str()) {
            // Inhalt bis zum passenden Ende-Tag überspringen; das Ende-Tag selbst schließt.
            let close = format!("</{name}");
            let skipped = find_ci(&b[i..], close.as_bytes()).unwrap_or(b.len() - i);
            i += skipped;
            depth -= 1;
        }
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_ci(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

pub fn extract(html: &str, url: &Url) -> Result<Extracted, ExtractError> {
    if exceeds_max_depth(html) {
        return Err(ExtractError::TooComplex);
    }
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
        lang: detect_language(&content_text),
        title,
        content_html: sanitize(&article.content),
        excerpt: truncate_chars(&excerpt, MAX_EXCERPT_CHARS),
        content_text,
    })
}

/// Höchstens so viele Zeichen des Textes werden zur Spracherkennung verwendet.
const LANG_SAMPLE_CHARS: usize = 4000;

/// Erkennt die Sprache des Textes mit `whatlang` (Trigramme, ohne Modelldateien) und liefert
/// den ISO-639-1-Code, wie ihn das HTML-Attribut `lang` braucht. Bei unsicherem Ergebnis
/// (kurzer oder gemischter Text) oder einer Sprache ohne zweistelligen Code gibt es `None`:
/// Eine falsche Angabe wäre schlechter als keine.
pub fn detect_language(text: &str) -> Option<String> {
    let end = text
        .char_indices()
        .nth(LANG_SAMPLE_CHARS)
        .map_or(text.len(), |(i, _)| i);
    let info = whatlang::detect(&text[..end])?;
    if !info.is_reliable() {
        return None;
    }
    iso639_1(info.lang().code()).map(str::to_owned)
}

/// ISO 639-3 (whatlang) nach ISO 639-1 (HTML `lang`); alle Sprachen, die whatlang kennt.
fn iso639_1(code: &str) -> Option<&'static str> {
    Some(match code {
        "epo" => "eo",
        "eng" => "en",
        "rus" => "ru",
        "cmn" => "zh",
        "spa" => "es",
        "por" => "pt",
        "ita" => "it",
        "ben" => "bn",
        "fra" => "fr",
        "deu" => "de",
        "ukr" => "uk",
        "kat" => "ka",
        "ara" => "ar",
        "hin" => "hi",
        "jpn" => "ja",
        "heb" => "he",
        "yid" => "yi",
        "pol" => "pl",
        "amh" => "am",
        "jav" => "jv",
        "kor" => "ko",
        "nob" => "nb",
        "dan" => "da",
        "swe" => "sv",
        "fin" => "fi",
        "tur" => "tr",
        "nld" => "nl",
        "hun" => "hu",
        "ces" => "cs",
        "ell" => "el",
        "bul" => "bg",
        "bel" => "be",
        "mar" => "mr",
        "kan" => "kn",
        "ron" => "ro",
        "slv" => "sl",
        "hrv" => "hr",
        "srp" => "sr",
        "mkd" => "mk",
        "lit" => "lt",
        "lav" => "lv",
        "est" => "et",
        "tam" => "ta",
        "vie" => "vi",
        "urd" => "ur",
        "tha" => "th",
        "guj" => "gu",
        "uzb" => "uz",
        "pan" => "pa",
        "aze" => "az",
        "ind" => "id",
        "tel" => "te",
        "pes" => "fa",
        "mal" => "ml",
        "ori" => "or",
        "mya" => "my",
        "nep" => "ne",
        "sin" => "si",
        "khm" => "km",
        "tuk" => "tk",
        "aka" => "ak",
        "zul" => "zu",
        "sna" => "sn",
        "afr" => "af",
        "lat" => "la",
        "slk" => "sk",
        "cat" => "ca",
        "tgl" => "tl",
        "hye" => "hy",
        "cym" => "cy",
        _ => return None,
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

    fn nested(depth: usize) -> String {
        format!(
            "<html><body>{}{}{}</body></html>",
            "<div>".repeat(depth),
            paragraphs(),
            "</div>".repeat(depth)
        )
    }

    #[test]
    fn tiefenpruefung_kommt_mit_kaputtem_html_und_rohtext_klar() {
        // Unvollständige Tags, Kommentare, Skripte mit „<“ und viele nicht geschlossene <li>/<p>.
        let flach = format!(
            "<!-- <div><div> --><script>if (a<b) {{ x('<div>') }}</script><ul>{}</ul><p>a<p>b<br><img alt=\"a>b\">< 3 <",
            "<li>Punkt".repeat(2_000)
        );
        assert!(!exceeds_max_depth(&flach));
        assert!(!exceeds_max_depth(""));
        assert!(!exceeds_max_depth("<div"));
        assert!(exceeds_max_depth(&"<div>".repeat(MAX_DEPTH + 1)));
        assert!(!exceeds_max_depth(&"<div>".repeat(MAX_DEPTH)));
    }

    #[test]
    fn normale_verschachtelung_ist_erlaubt() {
        let out = extract(&nested(100), &base()).unwrap();
        assert!(out.content_text.contains("ausführlicher Absatz"));
    }

    #[test]
    fn extrem_tiefe_seiten_werden_schnell_abgelehnt() {
        let start = std::time::Instant::now();
        for depth in [MAX_DEPTH + 10, 5_000, 25_000] {
            assert_eq!(
                extract(&nested(depth), &base()).unwrap_err(),
                ExtractError::TooComplex,
                "Tiefe {depth}"
            );
        }
        assert!(start.elapsed().as_secs() < 10, "Prüfung dauert zu lange");
    }

    #[test]
    fn tiefe_ohne_schliessende_tags_wird_abgelehnt() {
        let html = format!("<html><body>{}<p>Text</p>", "<span>".repeat(20_000));
        assert_eq!(
            extract(&html, &base()).unwrap_err(),
            ExtractError::TooComplex
        );
    }

    #[test]
    fn erkennt_die_sprache_des_textes() {
        let de = "Die Bundesregierung hat am Mittwoch ein neues Gesetz beschlossen, das den Ausbau \
                  der erneuerbaren Energien beschleunigen soll. Kritiker bemängeln, dass die \
                  Finanzierung noch nicht geklärt sei und viele Fragen offen blieben.";
        let en = "The government announced on Wednesday that it would speed up the expansion of \
                  renewable energy. Critics say the funding has not yet been settled and that \
                  many questions remain unanswered about how the plan will work in practice.";
        assert_eq!(detect_language(de).as_deref(), Some("de"));
        assert_eq!(detect_language(en).as_deref(), Some("en"));
        // Zu kurz, leer oder ohne Buchstaben: unbekannt statt geraten.
        assert_eq!(detect_language("Ja"), None);
        assert_eq!(detect_language(""), None);
        assert_eq!(detect_language("1234 5678 !!! ???"), None);
    }

    #[test]
    fn jede_erkannte_sprache_hat_einen_gueltigen_code() {
        for lang in whatlang::Lang::all() {
            let code = iso639_1(lang.code())
                .unwrap_or_else(|| panic!("kein ISO-639-1-Code für {}", lang.code()));
            assert!((2..=3).contains(&code.len()) && code == code.to_lowercase());
        }
    }

    #[test]
    fn extract_liefert_die_sprache() {
        let out = extract(&page("Titel", &paragraphs()), &base()).unwrap();
        assert_eq!(out.lang.as_deref(), Some("de"));
    }
}
