// Zeigt bereinigtes Artikel-HTML an, ohne innerHTML zu verwenden.
//
// Das Backend bereinigt den Inhalt bereits (ammonia). Hier wird er zusätzlich per
// Positivliste neu aufgebaut: Das HTML wird in einem inaktiven Dokument geparst
// (DOMParser führt keine Skripte aus und lädt nichts nach) und nur erlaubte Tags
// und Attribute werden mit createElement in die Seite übernommen.

const ALLOWED_TAGS = new Set([
  'p', 'br', 'hr', 'div', 'span',
  'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
  'a', 'img', 'figure', 'figcaption',
  'ul', 'ol', 'li', 'dl', 'dt', 'dd',
  'blockquote', 'q', 'cite', 'pre', 'code', 'kbd', 'samp', 'var',
  'em', 'strong', 'b', 'i', 'u', 's', 'del', 'ins', 'mark', 'small', 'sub', 'sup', 'abbr', 'time',
  'table', 'caption', 'thead', 'tbody', 'tfoot', 'tr', 'th', 'td',
]);

// Diese Elemente werden samt Inhalt verworfen. Unbekannte andere Tags werden
// ausgepackt (nur ihre Kinder bleiben).
const DROPPED_TAGS = new Set([
  'script', 'style', 'iframe', 'frame', 'frameset', 'object', 'embed', 'applet',
  'form', 'input', 'button', 'select', 'textarea', 'option',
  'svg', 'math', 'template', 'noscript', 'link', 'meta', 'base', 'title', 'head',
  'video', 'audio', 'source', 'track', 'canvas', 'dialog',
]);

const LINK_PROTOCOLS = ['http:', 'https:', 'mailto:'];
const IMAGE_PROTOCOLS = ['http:', 'https:'];

/** Liefert die absolute URL, wenn das Protokoll erlaubt ist, sonst null. */
export function safeUrl(value, baseUrl, protocols) {
  // Leere Werte würden sonst zur Basis-URL aufgelöst.
  if (typeof value !== 'string' || value.trim() === '') return null;
  try {
    const url = new URL(value, baseUrl);
    return protocols.includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

function copyAttributes(source, target, tag, baseUrl) {
  const title = source.getAttribute('title');
  if (title) target.setAttribute('title', title);

  if (tag === 'a') {
    const href = safeUrl(source.getAttribute('href') ?? '', baseUrl, LINK_PROTOCOLS);
    if (href) {
      target.setAttribute('href', href);
      target.setAttribute('rel', 'noopener noreferrer nofollow');
      target.setAttribute('target', '_blank');
    }
  } else if (tag === 'img') {
    const src = safeUrl(source.getAttribute('src') ?? '', baseUrl, IMAGE_PROTOCOLS);
    if (!src) return false; // Bild ohne brauchbare Quelle weglassen
    target.setAttribute('src', src);
    target.setAttribute('alt', source.getAttribute('alt') ?? '');
    target.setAttribute('loading', 'lazy');
    target.setAttribute('decoding', 'async');
    target.setAttribute('referrerpolicy', 'no-referrer');
  } else if (tag === 'td' || tag === 'th') {
    for (const name of ['colspan', 'rowspan']) {
      const n = Number.parseInt(source.getAttribute(name) ?? '', 10);
      if (Number.isInteger(n) && n >= 1 && n <= 100) target.setAttribute(name, String(n));
    }
  }
  return true;
}

function convertChildren(parent, baseUrl) {
  const result = [];
  for (const node of parent.childNodes) {
    const converted = convertNode(node, baseUrl);
    if (Array.isArray(converted)) result.push(...converted);
    else if (converted) result.push(converted);
  }
  return result;
}

function convertNode(node, baseUrl) {
  if (node.nodeType === Node.TEXT_NODE) return document.createTextNode(node.nodeValue);
  if (node.nodeType !== Node.ELEMENT_NODE) return null; // Kommentare usw.

  const tag = node.tagName.toLowerCase();
  if (DROPPED_TAGS.has(tag)) return null;
  if (!ALLOWED_TAGS.has(tag)) return convertChildren(node, baseUrl);

  const element = document.createElement(tag);
  if (!copyAttributes(node, element, tag, baseUrl)) return null;
  element.append(...convertChildren(node, baseUrl));
  return element;
}

/** Ersetzt den Inhalt von `container` durch die bereinigte Darstellung von `html`. */
export function renderSafeHtml(container, html, baseUrl) {
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  container.replaceChildren(...convertChildren(parsed.body, baseUrl));
}
