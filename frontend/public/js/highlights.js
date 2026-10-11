// Markierungen und Notizen im Lesemodus.
//
// Verankerung: Eine Markierung speichert die Zeichenposition im dargestellten Text (alle
// Textknoten des Artikels hintereinander, in UTF-16-Einheiten wie bei JavaScript-Strings) und
// zusätzlich den markierten Wortlaut mit etwas Umgebung. Passt die Position nicht mehr zum
// Wortlaut, wird die Stelle über den Wortlaut wiedergefunden.
//
// Alles läuft über DOM-Methoden und textContent, nie über innerHTML. Farben stehen als
// CSS-Variable `--hl` am Element (per CSSOM, das die CSP erlaubt) und stammen aus einer
// geprüften Hex-Form (#rrggbb).

export const PRESET_COLORS = [
  { name: 'Gelb', value: '#ffd93d' },
  { name: 'Grün', value: '#6fcf97' },
  { name: 'Blau', value: '#56b4f5' },
  { name: 'Rosa', value: '#f78fb3' },
  { name: 'Orange', value: '#ffa94d' },
];

const CONTEXT_CHARS = 32;
const MAX_QUOTE_CHARS = 2000;
const MAX_NOTE_CHARS = 2000;
const TEXT_NODES = 4; // NodeFilter.SHOW_TEXT
const WIDE_QUERY = '(min-width: 72rem)';
const CUSTOM_KEY = 'leseliste.hl.custom';
const HIDDEN_KEY = 'leseliste.hl.hidden';
const HEX = /^#[0-9a-f]{6}$/i;
// In diesen Elementen dürfen keine Textstücke aus reinem Leerraum umhüllt werden.
const STRUCTURAL = new Set(['ul', 'ol', 'dl', 'table', 'thead', 'tbody', 'tfoot', 'tr', 'figure']);

// ---------------------------------------------------------------------------
// Text und Positionen
// ---------------------------------------------------------------------------

/** Alle Textknoten von `root` mit ihrer Position im Gesamttext. */
export function collectText(root) {
  const nodes = [];
  let pos = 0;
  const walker = document.createTreeWalker(root, TEXT_NODES);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.nodeValue.length;
    nodes.push({ node, start: pos, end: pos + length });
    pos += length;
  }
  return { nodes, text: nodes.map((entry) => entry.node.nodeValue).join('') };
}

function commonSuffix(a, b) {
  let n = 0;
  while (n < a.length && n < b.length && a[a.length - 1 - n] === b[b.length - 1 - n]) n += 1;
  return n;
}

function commonPrefix(a, b) {
  let n = 0;
  while (n < a.length && n < b.length && a[n] === b[n]) n += 1;
  return n;
}

/**
 * Bestimmt die Stelle einer Markierung im Text: zuerst über die gespeicherte Position, sonst
 * über den Wortlaut (bei mehreren Treffern gewinnt die beste Übereinstimmung der Umgebung).
 * Liefert `null`, wenn der Wortlaut nicht mehr vorkommt.
 */
export function locate(text, h) {
  if (text.slice(h.start, h.end) === h.quote) return { start: h.start, end: h.end };
  let best = null;
  let bestScore = -1;
  let from = 0;
  for (let tries = 0; tries < 2000; tries += 1) {
    const i = text.indexOf(h.quote, from);
    if (i === -1) break;
    const before = text.slice(Math.max(0, i - h.prefix.length), i);
    const after = text.slice(i + h.quote.length, i + h.quote.length + h.suffix.length);
    const score = commonSuffix(before, h.prefix) + commonPrefix(after, h.suffix);
    const closer = best && Math.abs(i - h.start) < Math.abs(best.start - h.start);
    if (score > bestScore || (score === bestScore && closer)) {
      best = { start: i, end: i + h.quote.length };
      bestScore = score;
    }
    from = i + 1;
  }
  return best;
}

/** Zeichenposition eines Punktes (Knoten, Versatz) im Gesamttext von `root`. */
function offsetOf(root, node, offset) {
  const range = document.createRange();
  range.setStart(root, 0);
  range.setEnd(node, offset);
  return range.toString().length;
}

/**
 * Liest die aktuelle Textauswahl innerhalb von `root`.
 * Ergebnis: `null` (keine Auswahl), `{ error }` (nicht brauchbar) oder die Verankerung.
 */
export function selectionAnchor(root) {
  const selection = window.getSelection?.();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return null;

  const { text } = collectText(root);
  let start = offsetOf(root, range.startContainer, range.startOffset);
  let end = offsetOf(root, range.endContainer, range.endOffset);
  // Leerraum an den Rändern gehört nicht zur Markierung.
  while (start < end && /\s/.test(text[start])) start += 1;
  while (end > start && /\s/.test(text[end - 1])) end -= 1;
  if (end <= start) return null;
  if (end - start > MAX_QUOTE_CHARS) {
    return { error: `Die Auswahl ist zu lang (höchstens ${MAX_QUOTE_CHARS} Zeichen).` };
  }
  return {
    start,
    end,
    quote: text.slice(start, end),
    prefix: text.slice(Math.max(0, start - CONTEXT_CHARS), start),
    suffix: text.slice(end, end + CONTEXT_CHARS),
  };
}

// ---------------------------------------------------------------------------
// Markierungen im DOM
// ---------------------------------------------------------------------------

/** Entfernt alle Markierungen wieder (der Text bleibt). */
export function clearMarks(root) {
  for (const mark of root.querySelectorAll('mark.hl')) mark.replaceWith(...mark.childNodes);
  root.normalize();
}

function wrapSegment(node, from, to, h, first) {
  let target = node;
  if (to < target.length) target.splitText(to);
  if (from > 0) target = target.splitText(from);
  const mark = document.createElement('mark');
  mark.className = h.note ? 'hl has-note' : 'hl';
  mark.dataset.id = String(h.id);
  mark.style.setProperty('--hl', h.color);
  if (first) {
    mark.dataset.first = '1';
    mark.tabIndex = 0;
  }
  target.parentNode.insertBefore(mark, target);
  mark.append(target);
}

/**
 * Setzt die Markierungen in den Artikeltext. Liefert die Ids der Markierungen, deren Stelle
 * nicht mehr gefunden wurde.
 */
export function applyMarks(root, items) {
  clearMarks(root);
  const { text } = collectText(root);
  const lost = [];
  for (const h of items) {
    const place = locate(text, h);
    if (!place) {
      lost.push(h.id);
      continue;
    }
    // Nach jedem Umhüllen ändern sich die Textknoten, darum jedes Mal neu einsammeln.
    let first = true;
    for (const entry of collectText(root).nodes) {
      const from = Math.max(place.start, entry.start) - entry.start;
      const to = Math.min(place.end, entry.end) - entry.start;
      if (from >= to) continue;
      const piece = entry.node.nodeValue.slice(from, to);
      const parent = entry.node.parentNode.nodeName.toLowerCase();
      if (piece.trim() === '' && STRUCTURAL.has(parent)) continue;
      wrapSegment(entry.node, from, to, h, first);
      first = false;
    }
  }
  return lost;
}

// ---------------------------------------------------------------------------
// Oberfläche
// ---------------------------------------------------------------------------

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function readStore(key) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStore(key, value) {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    // Speicher nicht verfügbar: nur eine Bequemlichkeit, die Seite funktioniert auch so.
  }
}

function snippet(text, max) {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/**
 * Verbindet Lesemodus, Seitenleiste für Notizen und Bedienfeld.
 * `api` liefert listHighlights, createHighlight, updateHighlight, deleteHighlight;
 * `onAuthError(error)` meldet true, wenn die Sitzung abgelaufen ist.
 */
export function createHighlighter({ api, onAuthError }) {
  const $ = (id) => document.getElementById(id);
  const content = $('reader-content');
  const notes = $('reader-notes');
  const panel = $('hl-panel');
  const colorBox = $('hl-colors');
  const noteInput = $('hl-note');
  const toggle = $('hl-toggle');

  let articleId = null;
  let items = []; // Markierungen des geöffneten Artikels, nach Position sortiert
  let lost = new Set();
  let hidden = readStore(HIDDEN_KEY) === '1';
  let mode = null; // null | 'new' | 'edit'
  let pending = null; // Verankerung der aktuellen Auswahl (Modus 'new')
  let editId = null;
  let busy = false;
  let timer = null;
  let interactUntil = 0;
  let token = 0; // verwirft Antworten für einen inzwischen geschlossenen Artikel

  // ----- Farben -----
  function swatch(color, name) {
    const button = element('button', 'hl-swatch');
    button.type = 'button';
    button.dataset.color = color;
    button.title = name;
    button.setAttribute('aria-label', name);
    button.style.setProperty('--hl', color);
    return button;
  }

  const customInput = element('input', 'hl-custom-input');
  customInput.type = 'color';
  customInput.value = '#c084fc';
  customInput.setAttribute('aria-label', 'Eigene Farbe wählen');
  const customLabel = element('label', 'hl-custom', 'Eigene');
  customLabel.append(customInput);
  let customSwatch = null;

  function buildColors() {
    colorBox.replaceChildren();
    for (const preset of PRESET_COLORS) colorBox.append(swatch(preset.value, preset.name));
    const remembered = readStore(CUSTOM_KEY);
    customSwatch = null;
    if (remembered && HEX.test(remembered)) {
      customSwatch = swatch(remembered.toLowerCase(), 'Zuletzt gewählte eigene Farbe');
      colorBox.append(customSwatch);
      customInput.value = remembered.toLowerCase();
    }
    colorBox.append(customLabel);
  }

  function markActiveColor() {
    const current = mode === 'edit' ? items.find((h) => h.id === editId)?.color : null;
    for (const button of colorBox.querySelectorAll('.hl-swatch')) {
      button.setAttribute('aria-pressed', String(button.dataset.color === current));
    }
  }

  // ----- Anzeige -----
  function marksOf(id) {
    return [...content.querySelectorAll(`mark.hl[data-id="${Number(id)}"]`)];
  }

  /** Passen die Randnotizen noch neben den Text? (Bei breitem Text und schmalem Fenster nicht.) */
  function hasRoomForMargin() {
    const body = notes.parentElement?.getBoundingClientRect();
    if (!body || body.width === 0) return true; // ohne Layout (z. B. Tests) gilt nur die Fensterbreite
    const rem = parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
    return document.documentElement.clientWidth - body.right >= 16 * rem;
  }

  function layoutNotes() {
    const wide = Boolean(window.matchMedia?.(WIDE_QUERY).matches) && hasRoomForMargin();
    notes.classList.toggle('notes-margin', wide);
    const cards = [...notes.querySelectorAll('.note')];
    if (!wide) {
      for (const card of cards) card.style.top = '';
      notes.style.minHeight = '';
      return;
    }
    const base = notes.getBoundingClientRect().top;
    let bottom = 0;
    for (const card of cards) {
      const mark = marksOf(card.dataset.id)[0];
      const wanted = mark ? mark.getBoundingClientRect().top - base : bottom;
      const top = Math.max(wanted, bottom);
      card.style.top = `${top}px`;
      bottom = top + card.offsetHeight + 8;
    }
    notes.style.minHeight = `${bottom}px`;
  }

  function buildNotes() {
    notes.replaceChildren();
    for (const h of items) {
      const isLost = lost.has(h.id);
      if (!h.note && !isLost) continue;
      const card = element('button', 'note');
      card.type = 'button';
      card.dataset.id = String(h.id);
      card.style.setProperty('--hl', h.color);
      card.append(element('span', 'note-quote', snippet(h.quote, 90)));
      if (isLost) card.append(element('span', 'note-lost', 'Textstelle nicht mehr gefunden'));
      if (h.note) card.append(element('span', 'note-text', h.note));
      card.setAttribute('aria-label', `Markierung bearbeiten: ${snippet(h.quote, 40)}`);
      notes.append(card);
    }
    notes.hidden = hidden || notes.childElementCount === 0;
  }

  function render() {
    content.classList.toggle('hl-off', hidden);
    lost = new Set(applyMarks(content, items));
    buildNotes();
    layoutNotes();
    markEditing();
    toggle.hidden = items.length === 0;
    toggle.textContent = hidden ? 'Markierungen einblenden' : 'Markierungen ausblenden';
    toggle.setAttribute('aria-pressed', String(hidden));
  }

  function markEditing() {
    for (const mark of content.querySelectorAll('mark.hl-active')) mark.classList.remove('hl-active');
    for (const card of notes.querySelectorAll('.note-active')) card.classList.remove('note-active');
    if (mode !== 'edit') return;
    for (const mark of marksOf(editId)) mark.classList.add('hl-active');
    notes.querySelector(`.note[data-id="${Number(editId)}"]`)?.classList.add('note-active');
  }

  function showPanelError(message) {
    const box = $('hl-error');
    box.textContent = message;
    box.hidden = !message;
  }

  // ----- Bedienfeld -----
  function closePanel() {
    mode = null;
    pending = null;
    editId = null;
    panel.hidden = true;
    showPanelError('');
    markEditing();
  }

  function openNew(anchor) {
    mode = 'new';
    editId = null;
    pending = anchor.error ? null : anchor;
    $('hl-title').textContent = 'Auswahl markieren';
    $('hl-quote').textContent = anchor.error ? '' : snippet(anchor.quote, 140);
    $('hl-edit').hidden = true;
    showPanelError(anchor.error ?? '');
    buildColors();
    for (const button of colorBox.querySelectorAll('button')) button.disabled = Boolean(anchor.error);
    customInput.disabled = Boolean(anchor.error);
    markActiveColor();
    panel.hidden = false;
    markEditing();
  }

  function openEdit(id) {
    const h = items.find((item) => item.id === id);
    if (!h) return;
    mode = 'edit';
    editId = id;
    pending = null;
    $('hl-title').textContent = 'Markierung';
    $('hl-quote').textContent = snippet(h.quote, 140);
    $('hl-edit').hidden = false;
    noteInput.value = h.note ?? '';
    showPanelError('');
    buildColors();
    for (const button of colorBox.querySelectorAll('button')) button.disabled = false;
    customInput.disabled = false;
    markActiveColor();
    panel.hidden = false;
    markEditing();
  }

  async function run(task) {
    if (busy) return;
    busy = true;
    const mine = token;
    showPanelError('');
    try {
      await task();
    } catch (error) {
      if (mine !== token || onAuthError(error)) return;
      showPanelError(error.message);
    } finally {
      busy = false;
    }
  }

  async function pickColor(color) {
    if (!HEX.test(color)) return;
    const value = color.toLowerCase();
    if (mode === 'new' && pending) {
      const anchor = pending;
      await run(async () => {
        const created = await api.createHighlight(articleId, { ...anchor, color: value });
        items = [...items, created].sort((a, b) => a.start - b.start || a.id - b.id);
        window.getSelection?.()?.removeAllRanges();
        render();
        openEdit(created.id);
      });
    } else if (mode === 'edit') {
      const id = editId;
      await run(async () => {
        const updated = await api.updateHighlight(id, { color: value });
        items = items.map((h) => (h.id === id ? updated : h));
        render();
        openEdit(id);
      });
    }
  }

  function rememberCustom(color) {
    writeStore(CUSTOM_KEY, color.toLowerCase());
  }

  colorBox.addEventListener('click', (event) => {
    const button = event.target.closest('.hl-swatch');
    if (button) pickColor(button.dataset.color);
  });
  customInput.addEventListener('change', () => {
    rememberCustom(customInput.value);
    pickColor(customInput.value);
  });

  $('hl-save').addEventListener('click', () => {
    if (mode !== 'edit') return;
    const id = editId;
    const text = noteInput.value.trim();
    if (text.length > MAX_NOTE_CHARS) {
      showPanelError(`Die Notiz ist zu lang (höchstens ${MAX_NOTE_CHARS} Zeichen).`);
      return;
    }
    run(async () => {
      const updated = await api.updateHighlight(id, { note: text === '' ? null : text });
      items = items.map((h) => (h.id === id ? updated : h));
      render();
      openEdit(id);
    });
  });

  $('hl-delete').addEventListener('click', () => {
    if (mode !== 'edit') return;
    const id = editId;
    run(async () => {
      await api.deleteHighlight(id);
      items = items.filter((h) => h.id !== id);
      closePanel();
      render();
    });
  });

  $('hl-close').addEventListener('click', closePanel);
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !panel.hidden) closePanel();
  });

  // Das Bedienfeld soll die Textauswahl nicht aufheben (Maus), und kurz nach einer
  // Berührung wird die Auswahl nicht als „aufgehoben“ gewertet.
  panel.addEventListener('mousedown', (event) => {
    interactUntil = Date.now() + 800;
    if (!event.target.closest('textarea, input')) event.preventDefault();
  });
  panel.addEventListener('touchstart', () => {
    interactUntil = Date.now() + 800;
  }, { passive: true });

  // ----- Auswahl und Klicks im Text -----
  function onSelectionChange() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (articleId === null || hidden) return;
      const anchor = selectionAnchor(content);
      if (anchor) {
        openNew(anchor);
      } else if (mode === 'new' && !panel.contains(document.activeElement) && Date.now() > interactUntil) {
        closePanel();
      }
    }, 120);
  }
  document.addEventListener('selectionchange', onSelectionChange);

  function openFromMark(event) {
    const mark = event.target.closest?.('mark.hl');
    if (!mark || hidden) return;
    const selection = window.getSelection?.();
    if (selection && !selection.isCollapsed) return; // der Benutzer markiert gerade Text
    // Bei verschachtelten Markierungen zählt die innerste.
    openEdit(Number(mark.dataset.id));
  }
  content.addEventListener('click', openFromMark);
  content.addEventListener('keydown', (event) => {
    if ((event.key === 'Enter' || event.key === ' ') && event.target.matches?.('mark.hl')) {
      event.preventDefault();
      openFromMark(event);
    }
  });

  notes.addEventListener('click', (event) => {
    const card = event.target.closest('.note');
    if (!card) return;
    const id = Number(card.dataset.id);
    openEdit(id);
    marksOf(id)[0]?.scrollIntoView?.({ block: 'center', behavior: 'smooth' });
  });

  toggle.addEventListener('click', () => {
    hidden = !hidden;
    writeStore(HIDDEN_KEY, hidden ? '1' : null);
    if (hidden) closePanel();
    render();
  });

  window.addEventListener('resize', () => layoutNotes());
  // Schrift, Größe oder Breite ändern die Lage der Markierungen (js/reading.js).
  window.addEventListener('leseliste:display', () => layoutNotes());
  // Bilder verschieben den Text nach unten, wenn sie fertig geladen sind.
  content.addEventListener('load', () => layoutNotes(), true);

  return {
    /**
     * Zeigt die Markierungen des Artikels, dessen Text schon angezeigt wird. Sind sie schon
     * geladen (`preloaded`, z. B. mit dem Artikel oder aus dem Offline-Speicher), entfällt die
     * Anfrage.
     */
    async open(id, preloaded) {
      this.reset();
      articleId = id;
      const mine = token;
      try {
        const list = Array.isArray(preloaded) ? preloaded : await api.listHighlights(id);
        if (mine !== token) return;
        items = [...list].sort((a, b) => a.start - b.start || a.id - b.id);
        render();
      } catch (error) {
        if (mine !== token || onAuthError(error)) return;
        // Der Artikel bleibt lesbar, auch wenn die Markierungen nicht geladen werden.
        panel.hidden = false;
        $('hl-title').textContent = 'Markierungen';
        showPanelError(`Markierungen konnten nicht geladen werden: ${error.message}`);
        mode = null;
      }
    },
    /** Räumt alles auf (beim Verlassen oder vor dem Laden eines anderen Artikels). */
    reset() {
      token += 1;
      articleId = null;
      items = [];
      lost = new Set();
      busy = false;
      clearTimeout(timer);
      closePanel();
      notes.replaceChildren();
      notes.hidden = true;
      toggle.hidden = true;
    },
  };
}
