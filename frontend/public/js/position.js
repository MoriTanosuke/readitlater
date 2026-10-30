// Leseposition: merkt sich still, wo der Benutzer im Artikel steht, und springt beim erneuten
// Öffnen dorthin zurück. Es gibt keine Bedienelemente und keine Meldungen; Fehler beim
// Speichern dürfen das Lesen nie stören (das übernimmt der Aufrufer in `save`).
//
// Die Position ist ein Textoffset (UTF-16-Einheiten über den dargestellten Artikeltext, dieselbe
// Zählung wie bei den Markierungen), keine Pixelzahl. Dadurch stimmt sie auch dann, wenn
// Schrift, Textbreite oder Gerät wechseln. Gemessen wird nur über Blockelemente (Absätze,
// Überschriften, Listenpunkte …) und anteilig darin; das ist auf eine Zeile genau genug und
// kommt ohne Messen einzelner Zeichen aus.
//
// Regeln:
// - Gespeichert wird nur bei ungelesenen Artikeln (`enabled`). Gelesene starten immer oben,
//   das erzwingt auch das Backend.
// - Ist das Ende des Textes im Fenster zu sehen, gilt der Artikel als zu Ende gelesen und die
//   Position wird 0 (beim nächsten Öffnen wieder oben).
// - Beim Scrollen wird höchstens einmal pro Sekunde gespeichert, beim Verlassen der Seite
//   (Navigation, Tab im Hintergrund, Schließen) sofort.

import { collectText } from './highlights.js';

const LINE_PX = 24; // Lesezeile: so weit unter dem Fensterrand steht die gemerkte Stelle
const END_TOLERANCE_PX = 4; // so nah am Ende des Textes zählt der Artikel als zu Ende gelesen
const SAVE_DELAY_MS = 1000;
const MIN_DELTA = 30; // kleinere Verschiebungen (Zeichen) sind kein Speichern wert
const QUIET_MS = 400; // so lange stammen Scroll-Ereignisse von der Wiederherstellung, nicht vom Benutzer
const INPUT_EVENTS = ['wheel', 'touchstart', 'keydown', 'pointerdown', 'mousedown'];

// Elemente, die eigene Zeilen belegen. Text in anderen Elementen (a, em, mark …) gehört zum
// nächsten Blockelement darüber.
const BLOCKS = new Set([
  'p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'li', 'blockquote', 'pre', 'figcaption', 'td', 'th',
  'dt', 'dd', 'div', 'section', 'article', 'header', 'footer', 'aside', 'main', 'figure',
  'table', 'ul', 'ol', 'dl', 'details', 'summary',
]);

// ---------------------------------------------------------------------------
// Text und Blöcke
// ---------------------------------------------------------------------------

function blockOf(node, root) {
  let element = node.parentElement;
  while (element && element !== root && !BLOCKS.has(element.localName)) element = element.parentElement;
  return element ?? root;
}

/**
 * Zusammenhängende Textstücke je Blockelement mit ihrem Bereich im Gesamttext:
 * `[{ el, start, end }]` in Lesereihenfolge. Leerraum zwischen Blöcken zählt nicht mit.
 */
export function textBlocks(root) {
  const blocks = [];
  for (const { node, start, end } of collectText(root).nodes) {
    if (!/\S/.test(node.nodeValue)) continue;
    const el = blockOf(node, root);
    const last = blocks[blocks.length - 1];
    if (last && last.el === el) last.end = end;
    else blocks.push({ el, start, end });
  }
  return blocks;
}

function isRendered(rect) {
  return rect.height > 0 || rect.width > 0;
}

/**
 * Textoffset an der Zeile `y` (Fensterkoordinate): der Anfang des ersten Blocks, der nicht
 * komplett darüber liegt, oder anteilig darin, wenn die Zeile mitten im Block liegt.
 * Der Artikelanfang ist immer 0.
 */
export function offsetAtLine(root, y) {
  const blocks = textBlocks(root);
  for (const [index, block] of blocks.entries()) {
    const rect = block.el.getBoundingClientRect();
    if (!isRendered(rect) || rect.bottom <= y) continue;
    if (rect.top >= y) return index === 0 ? 0 : block.start;
    const fraction = (y - rect.top) / rect.height;
    return block.start + Math.floor(fraction * (block.end - block.start));
  }
  return blocks.length ? blocks[blocks.length - 1].end : 0;
}

/** Scrollt so, dass der Textoffset an der Zeile `y` steht. Gibt `false` zurück, wenn es keinen Text gibt. */
export function scrollToOffset(root, offset, y) {
  const blocks = textBlocks(root);
  if (blocks.length === 0) return false;
  const block = blocks.find((b) => b.end > offset) ?? blocks[blocks.length - 1];
  const rect = block.el.getBoundingClientRect();
  if (!isRendered(rect)) return false;
  const fraction = (offset - block.start) / Math.max(1, block.end - block.start);
  const target = rect.top + Math.min(1, Math.max(0, fraction)) * rect.height;
  window.scrollTo({ top: Math.max(0, window.scrollY + target - y), left: 0, behavior: 'instant' });
  return true;
}

/** Ist das Ende des Textes im Fenster zu sehen? `null`, wenn der Text gerade nicht dargestellt ist. */
export function reachedEnd(root) {
  const rect = root.getBoundingClientRect();
  if (!isRendered(rect)) return null;
  const viewport = document.documentElement.clientHeight || window.innerHeight;
  return rect.bottom <= viewport + END_TOLERANCE_PX;
}

// ---------------------------------------------------------------------------
// Verfolgen
// ---------------------------------------------------------------------------

/**
 * `root`: Element mit dem Artikeltext. `save(id, pos, { keepalive })` speichert die Position
 * (async, Fehler behandelt der Aufruf selbst). `now` ist nur für Tests austauschbar.
 */
export function createPositionTracker({ root, save, now = () => Date.now() }) {
  let session = null; // { id, enabled, target, lastSaved, interacted, dirty, quietUntil }
  let timer = null;
  let frame = null;

  function currentPosition() {
    const ended = reachedEnd(root);
    if (ended === null) return null;
    return ended ? 0 : offsetAtLine(root, LINE_PX);
  }

  /** Speichert die aktuelle Position, falls der Benutzer gescrollt hat und sie sich spürbar geändert hat. */
  function flush({ keepalive = false } = {}) {
    clearTimeout(timer);
    timer = null;
    if (!session?.enabled || !session.dirty) return;
    session.dirty = false;
    const pos = currentPosition();
    if (pos === null || pos === session.lastSaved) return;
    // Nach 0 (Anfang, Ende) wird immer gespeichert, sonst nur bei spürbarer Verschiebung.
    if (pos !== 0 && Math.abs(pos - session.lastSaved) < MIN_DELTA) return;
    session.lastSaved = pos;
    save(session.id, pos, { keepalive });
  }

  function quiet() {
    session.quietUntil = now() + QUIET_MS;
  }

  /** Springt zur Position; `pos` 0 heißt nach oben. */
  function restore(pos) {
    session.target = pos;
    session.lastSaved = pos;
    quiet();
    if (pos > 0) scrollToOffset(root, pos, LINE_PX);
    else window.scrollTo({ top: 0, left: 0, behavior: 'instant' });
  }

  function onScroll() {
    if (!session?.enabled) return;
    if (!session.interacted && now() < session.quietUntil) return; // Sprung der Wiederherstellung
    session.dirty = true;
    if (!timer) timer = setTimeout(() => flush(), SAVE_DELAY_MS);
  }

  function onInput() {
    if (session) session.interacted = true;
  }

  // Bilder ohne feste Größe und nachladende Webfonts (Literata, Inter) verschieben den Text:
  // Die gemerkte Stelle wird nachgeführt, solange der Benutzer noch nichts getan hat.
  function onImageLoad() {
    if (!session || session.interacted || session.target <= 0 || frame !== null) return;
    frame = window.requestAnimationFrame(() => {
      frame = null;
      if (!session || session.interacted) return;
      quiet();
      scrollToOffset(root, session.target, LINE_PX);
    });
  }

  function onHide() {
    if (document.visibilityState === 'hidden') flush({ keepalive: true });
  }

  function onPageHide() {
    flush({ keepalive: true });
  }

  function attach() {
    window.addEventListener('scroll', onScroll, { passive: true });
    for (const name of INPUT_EVENTS) window.addEventListener(name, onInput, { passive: true, capture: true });
    root.addEventListener('load', onImageLoad, true);
    document.fonts?.addEventListener?.('loadingdone', onImageLoad); // fehlt in älteren Umgebungen und jsdom
    document.addEventListener('visibilitychange', onHide);
    window.addEventListener('pagehide', onPageHide);
  }

  function detach() {
    window.removeEventListener('scroll', onScroll);
    for (const name of INPUT_EVENTS) window.removeEventListener(name, onInput, true);
    root.removeEventListener('load', onImageLoad, true);
    document.fonts?.removeEventListener?.('loadingdone', onImageLoad);
    document.removeEventListener('visibilitychange', onHide);
    window.removeEventListener('pagehide', onPageHide);
    clearTimeout(timer);
    timer = null;
    if (frame !== null) window.cancelAnimationFrame(frame);
    frame = null;
  }

  return {
    /** Beginnt die Aufzeichnung für den dargestellten Artikel und springt zur gemerkten Stelle. */
    begin(article) {
      this.end();
      const pos = article.read_pos ?? 0;
      session = {
        id: article.id,
        enabled: !article.is_read,
        target: pos,
        lastSaved: pos,
        interacted: false,
        dirty: false,
        quietUntil: 0,
      };
      attach();
      if (pos > 0) restore(pos);
    },

    /**
     * Antwort des Servers für den bereits dargestellten Artikel: übernimmt dessen Status und
     * Position, solange der Benutzer noch nichts getan hat (die Kopie auf dem Gerät kann
     * älter sein, etwa wenn auf einem anderen Gerät weitergelesen wurde).
     */
    adopt(article) {
      if (session?.id !== article.id) return;
      session.enabled = !article.is_read;
      const pos = article.read_pos ?? 0;
      if (!session.interacted && pos !== session.target) restore(pos);
    },

    /** Gelesen markiert (aus) oder wieder ungelesen (an). Eine vorgemerkte Speicherung entfällt. */
    setEnabled(enabled) {
      if (!session) return;
      session.enabled = enabled;
      session.dirty = false;
      session.lastSaved = 0; // gelesene Artikel haben im Backend die Position 0
      clearTimeout(timer);
      timer = null;
    },

    /** Beendet die Aufzeichnung und speichert die letzte Position. Aufrufen, bevor der Text verschwindet. */
    end() {
      if (!session) return;
      flush({ keepalive: true });
      detach();
      session = null;
    },

    /** Beendet die Aufzeichnung ohne zu speichern (Sitzung beendet). */
    cancel() {
      if (!session) return;
      detach();
      session = null;
    },

    flush,
  };
}
