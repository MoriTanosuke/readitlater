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
// Lokal abgelegte Bilder liefert nur unsere eigene Schnittstelle, und zwar in genau dieser Form.
const LOCAL_IMAGE = /^\/api\/articles\/\d+\/images\/\d+$/;

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

function copyAttributes(source, target, tag, ctx) {
  const title = source.getAttribute('title');
  if (title) target.setAttribute('title', title);

  if (tag === 'a') {
    const href = safeUrl(source.getAttribute('href') ?? '', ctx.baseUrl, LINK_PROTOCOLS);
    if (href) {
      target.setAttribute('href', href);
      target.setAttribute('rel', 'noopener noreferrer nofollow');
      target.setAttribute('target', '_blank');
    }
  } else if (tag === 'img') {
    const original = safeUrl(source.getAttribute('src') ?? '', ctx.baseUrl, IMAGE_PROTOCOLS);
    if (!original) return false; // Bild ohne brauchbare Quelle weglassen
    // Original-Adresse merken: Sie ist der Schlüssel für das lokale Bild (applyLocalImages).
    target.dataset.origin = original;
    const local = localImage(ctx.images, original);
    target.setAttribute('src', local ? local.src : original);
    if (local) setSize(target, local);
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

/** Das lokale Bild zur Original-Adresse, sofern die Angabe der Schnittstelle plausibel ist. */
function localImage(images, original) {
  if (!images || !Object.hasOwn(images, original)) return null;
  const entry = images[original];
  return entry && typeof entry.src === 'string' && LOCAL_IMAGE.test(entry.src) ? entry : null;
}

/** Breite und Höhe der verkleinerten Fassung: der Browser reserviert den Platz (kein Springen). */
function setSize(img, entry) {
  for (const name of ['width', 'height']) {
    const n = Number(entry[name]);
    if (Number.isInteger(n) && n >= 1 && n <= 10000) img.setAttribute(name, String(n));
  }
}

function convertChildren(parent, ctx) {
  const result = [];
  for (const node of parent.childNodes) {
    const converted = convertNode(node, ctx);
    if (Array.isArray(converted)) result.push(...converted);
    else if (converted) result.push(converted);
  }
  return result;
}

function convertNode(node, ctx) {
  if (node.nodeType === Node.TEXT_NODE) return document.createTextNode(node.nodeValue);
  if (node.nodeType !== Node.ELEMENT_NODE) return null; // Kommentare usw.

  const tag = node.tagName.toLowerCase();
  if (DROPPED_TAGS.has(tag)) return null;
  if (!ALLOWED_TAGS.has(tag)) return convertChildren(node, ctx);

  const element = document.createElement(tag);
  if (!copyAttributes(node, element, tag, ctx)) return null;
  element.append(...convertChildren(node, ctx));
  return element;
}

/**
 * Ersetzt den Inhalt von `container` durch die bereinigte Darstellung von `html`.
 * `images` ordnet Original-Adressen den lokal abgelegten, verkleinerten Bildern zu
 * (`{ "https://…/a.jpg": { src, width, height } }`, siehe `GET /api/articles/{id}`).
 */
export function renderSafeHtml(container, html, baseUrl, images = {}) {
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  container.replaceChildren(...convertChildren(parsed.body, { baseUrl, images }));
}

/**
 * Tauscht nachträglich Bilder, die inzwischen lokal vorliegen (der Worker war nach dem
 * Öffnen fertig), ohne den Text neu aufzubauen. Gibt die Zahl der Bilder zurück, die noch von
 * der Originaladresse geladen werden.
 */
export function applyLocalImages(container, images) {
  let remote = 0;
  for (const img of container.querySelectorAll('img[data-origin]')) {
    const local = localImage(images, img.dataset.origin);
    if (!local) {
      remote += 1;
      continue;
    }
    if (img.getAttribute('src') !== local.src) img.setAttribute('src', local.src);
    setSize(img, local);
  }
  return remote;
}
