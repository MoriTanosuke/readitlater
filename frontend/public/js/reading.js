// Leseeinstellungen: Schriftart, Textgröße, Textbreite, Zeilenabstand, Hell/Dunkel und Kontrast.
//
// Die Einstellungen gelten pro Gerät und liegen in localStorage (nur als Bequemlichkeit: ohne
// Speicher gilt die Voreinstellung). Angewendet werden sie über Attribute am <html>-Element
// (data-theme, data-contrast, data-reading-font, data-reading-align) und CSS-Variablen per CSSOM; wegen der CSP gibt
// es kein style-Attribut im HTML. Das Aussehen selbst steht in style.css.

const KEY = 'leseliste-reading';
const EVENT = 'leseliste:display';

export const FONTS = {
  sans: 'Systemschrift',
  serif: 'Serif',
  wide: 'Breite Schrift',
  mono: 'Monospace',
};
export const ALIGNS = { left: 'Linksbündig', justify: 'Blocksatz' };
export const THEMES = { auto: 'Wie das Gerät', light: 'Hell', dark: 'Dunkel' };
export const CONTRASTS = { soft: 'Weich', normal: 'Normal', high: 'Hoch' };

export const RANGES = {
  size: { min: 80, max: 200, step: 10 }, // Prozent
  width: { min: 40, max: 100, step: 5 }, // Zeichen pro Zeile
  leading: { min: 1.3, max: 2.2, step: 0.1 },
};

export const DEFAULTS = Object.freeze({
  font: 'sans',
  size: 100,
  width: 70,
  leading: 1.7,
  align: 'left',
  theme: 'auto',
  contrast: 'normal',
});

const BASE_SIZE_REM = 1.0625; // Textgröße im Lesemodus bei 100 %

/** Macht aus beliebigen (auch kaputten) Daten gültige Einstellungen. */
export function normalize(raw) {
  const input = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : {};
  const pick = (value, allowed, fallback) =>
    typeof value === 'string' && Object.hasOwn(allowed, value) ? value : fallback;
  const num = (value, { min, max, step }, fallback) => {
    if (typeof value !== 'number' || !Number.isFinite(value)) return fallback;
    const snapped = Math.round(value / step) * step;
    return Math.round(Math.min(max, Math.max(min, snapped)) * 100) / 100;
  };
  return {
    font: pick(input.font, FONTS, DEFAULTS.font),
    size: num(input.size, RANGES.size, DEFAULTS.size),
    width: num(input.width, RANGES.width, DEFAULTS.width),
    leading: num(input.leading, RANGES.leading, DEFAULTS.leading),
    align: pick(input.align, ALIGNS, DEFAULTS.align),
    theme: pick(input.theme, THEMES, DEFAULTS.theme),
    contrast: pick(input.contrast, CONTRASTS, DEFAULTS.contrast),
  };
}

function readStore() {
  try {
    return JSON.parse(window.localStorage.getItem(KEY) ?? 'null');
  } catch {
    return null; // kein Speicher oder kaputter Inhalt
  }
}

function writeStore(settings) {
  try {
    const isDefault = Object.keys(DEFAULTS).every((k) => settings[k] === DEFAULTS[k]);
    if (isDefault) window.localStorage.removeItem(KEY);
    else window.localStorage.setItem(KEY, JSON.stringify(settings));
  } catch {
    // Die Einstellung gilt dann nur bis zum Neuladen.
  }
}

let current = normalize(readStore());

export function getSettings() {
  return { ...current };
}

// Farben der Adressleiste (meta theme-color) bei erzwungenem Hell/Dunkel.
const THEME_COLORS = { light: '#1b5e9e', dark: '#161615' };
let originalMetas = null;

function applyThemeColor(theme) {
  const metas = [...document.querySelectorAll('meta[name="theme-color"]')];
  if (!originalMetas) originalMetas = metas.map((m) => m.getAttribute('content'));
  metas.forEach((meta, i) => {
    meta.setAttribute('content', theme === 'auto' ? originalMetas[i] : THEME_COLORS[theme]);
  });
}

/** Schreibt die Einstellungen in das Dokument. */
export function apply(settings = current) {
  const root = document.documentElement;
  root.dataset.theme = settings.theme;
  root.dataset.contrast = settings.contrast;
  root.dataset.readingFont = settings.font;
  root.dataset.readingAlign = settings.align;
  root.style.setProperty('--reading-size', `${(BASE_SIZE_REM * settings.size) / 100}rem`);
  root.style.setProperty('--reading-leading', String(settings.leading));
  root.style.setProperty('--reading-measure', String(settings.width));
  applyThemeColor(settings.theme);
  window.dispatchEvent(new window.CustomEvent(EVENT));
}

export function update(patch) {
  current = normalize({ ...current, ...patch });
  writeStore(current);
  apply();
}

export function reset() {
  current = { ...DEFAULTS };
  writeStore(current);
  apply();
}

// Ein anderer Tab hat etwas geändert.
window.addEventListener('storage', (event) => {
  if (event.key !== KEY && event.key !== null) return;
  current = normalize(readStore());
  apply();
});

apply();

// ---------------------------------------------------------------------------
// Bedienelemente (Lesemodus-Bedienfeld und Seite „Darstellung“)
// ---------------------------------------------------------------------------

const number = (value, digits = 0) => value.toFixed(digits).replace('.', ',');

const LABELS = {
  size: (v) => `${number(v)} %`,
  width: (v) => `${number(v)} Zeichen`,
  leading: (v) => number(v, 1),
};

/** Baut die Bedienelemente in `container`; sie bleiben mit den Einstellungen synchron. */
export function mountControls(container) {
  const el = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const controls = {};

  const selectField = (key, label, options) => {
    const wrap = el('label', 'reading-field');
    wrap.append(el('span', null, label));
    const select = el('select');
    for (const [value, text] of Object.entries(options)) {
      const option = el('option', null, text);
      option.value = value;
      select.append(option);
    }
    select.addEventListener('change', () => update({ [key]: select.value }));
    wrap.append(select);
    controls[key] = select;
    return wrap;
  };

  const rangeField = (key, label) => {
    const { min, max, step } = RANGES[key];
    const wrap = el('label', 'reading-field');
    const head = el('span', 'reading-head');
    const out = el('output', 'reading-value');
    head.append(el('span', null, label), out);
    const input = el('input');
    input.type = 'range';
    input.min = String(min);
    input.max = String(max);
    input.step = String(step);
    input.addEventListener('input', () => update({ [key]: Number(input.value) }));
    wrap.append(head, input);
    controls[key] = input;
    controls[`${key}Out`] = out;
    return wrap;
  };

  const fields = el('div', 'reading-fields');
  fields.append(
    selectField('font', 'Schriftart', FONTS),
    rangeField('size', 'Textgröße'),
    rangeField('width', 'Textbreite'),
    rangeField('leading', 'Zeilenabstand'),
    selectField('align', 'Textausrichtung', ALIGNS),
    selectField('theme', 'Farbschema', THEMES),
    selectField('contrast', 'Kontrast', CONTRASTS),
  );
  const resetButton = el('button', 'reading-reset', 'Zurücksetzen');
  resetButton.type = 'button';
  resetButton.addEventListener('click', () => reset());
  container.replaceChildren(fields, resetButton);

  const sync = () => {
    const s = getSettings();
    for (const key of Object.keys(DEFAULTS)) {
      const control = controls[key];
      if (String(control.value) !== String(s[key])) control.value = String(s[key]);
      const out = controls[`${key}Out`];
      if (out) out.textContent = LABELS[key](s[key]);
    }
    resetButton.disabled = Object.keys(DEFAULTS).every((k) => s[k] === DEFAULTS[k]);
  };
  window.addEventListener(EVENT, sync);
  sync();
}
