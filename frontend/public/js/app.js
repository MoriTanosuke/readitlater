import { api, ApiError } from './api.js';
import { createBackBubble } from './backlink.js';
import { createHighlighter } from './highlights.js';
import * as offline from './offline.js';
import { createPositionTracker } from './position.js';
import { mountControls } from './reading.js';
import { applyLocalImages, renderSafeHtml, safeUrl } from './safe-html.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['loading', 'auth', 'home', 'reader', 'password', 'delete', 'share', 'data', 'offline', 'display'];
const PAGE_SIZE = 50;
const APP_TITLE = 'Leseliste';
const SLOW_MS = 2500; // so lange wartet die Liste auf den Server, bevor gespeicherte Artikel erscheinen
const BOOT_SLOW_MS = 3000; // dasselbe beim Start der App

let mode = 'login'; // 'login' | 'register'
let currentUser = null;
let articles = []; // bereits geladene Artikel der Liste
let hasMore = false;
let listLoaded = false;
let listToken = 0; // verwirft veraltete Listenantworten
let filter = { q: '', read: '', tag: '' };
let tags = []; // Schlagwörter mit Anzahl
let tagsStale = true;
let pendingNotice = ''; // einmalige Meldung für die Liste (z. B. nach Passwortänderung)
let readerArticle = null;
let routeToken = 0; // verwirft veraltete Antworten bei schnellem Navigieren
let offlineView = false; // zeigt die App gerade gespeicherte statt aktuelle Daten?
let lastSync = 0; // Zeitpunkt (ms) des letzten Abgleichs mit dem Server
let pendingShare = takeSharedUrl(); // Adresse, die per „Teilen“ an die App gesendet wurde

const highlighter = createHighlighter({ api, onAuthError: (error) => handleAuthError(error) });
const position = createPositionTracker({ root: $('reader-content'), save: saveReadingPosition });
const backBubble = createBackBubble({
  bubble: $('reader-back-float'),
  top: $('reader-back'),
  bottom: $('reader-back-bottom'),
  content: $('reader-content'),
  isActive: () => !$('view-reader').hidden,
});

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

function show(view) {
  if (view !== 'reader') position.end(); // vor dem Ausblenden: danach lässt sich nichts mehr messen
  if (view !== 'reader') highlighter.reset();
  if (view !== 'reader') setReadingPanel(false);
  for (const name of VIEWS) $(`view-${name}`).hidden = name !== view;
  backBubble.reset(); // nach dem Umschalten: blendet aus und misst im Lesemodus neu
}

function showError(element, message) {
  element.textContent = message;
  element.hidden = false;
}

function clearError(element) {
  element.textContent = '';
  element.hidden = true;
}

async function withBusy(button, task) {
  button.disabled = true;
  try {
    return await task();
  } finally {
    button.disabled = false;
  }
}

function hostOf(url) {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

function formatDate(iso) {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleDateString('de-DE', { day: '2-digit', month: '2-digit', year: 'numeric' });
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text; // nie innerHTML
  return node;
}

/** Blendet den Hinweis „keine oder schlechte Verbindung“ ein oder aus. */
function setOfflineBanner(on) {
  offlineView = on;
  $('offline-banner').hidden = !on;
}

/** Fehler „Server nicht erreichbar“ (Status 0)? Dann hilft der Offline-Speicher. */
function isUnreachable(error) {
  return error instanceof ApiError && error.status === 0;
}

function showNotice(text) {
  const notice = $('home-notice');
  notice.textContent = text;
  notice.hidden = !text;
}

/**
 * Die Teilen-Funktion des Betriebssystems ruft die App mit ?url=…&text=…&title=… auf (Web Share
 * Target). Gesucht wird die erste http(s)-Adresse; die Parameter verschwinden aus der Adresszeile.
 */
function takeSharedUrl() {
  const params = new URLSearchParams(location.search);
  let found = '';
  for (const key of ['url', 'text', 'title']) {
    const match = /https?:\/\/[^\s<>"']+/i.exec(params.get(key) ?? '');
    if (match) {
      found = match[0];
      break;
    }
  }
  if (location.search) history.replaceState(null, '', location.pathname + location.hash);
  return found;
}

function resetHash() {
  history.replaceState(null, '', location.pathname + location.search);
}

/** Bei abgelaufener Sitzung zur Anmeldung zurückkehren. */
function handleAuthError(error) {
  if (error instanceof ApiError && error.status === 401) {
    endSession();
    return true;
  }
  return false;
}

function endSession() {
  position.cancel(); // ohne Sitzung lässt sich nichts mehr speichern
  offline.clearAll(); // gespeicherte Artikel gehören zur Sitzung und verschwinden mit ihr
  setOfflineBanner(false);
  lastSync = 0;
  currentUser = null;
  articles = [];
  hasMore = false;
  listLoaded = false;
  listToken += 1;
  filter = { q: '', read: '', tag: '' };
  tags = [];
  tagsStale = true;
  pendingNotice = '';
  readerArticle = null;
  routeToken += 1;
  document.title = APP_TITLE;
  resetHash();
  setMode('login');
  show('auth');
}

// ---------------------------------------------------------------------------
// Anmeldung und Registrierung
// ---------------------------------------------------------------------------

function setMode(next) {
  mode = next;
  const isLogin = next === 'login';
  $('tab-login').setAttribute('aria-selected', String(isLogin));
  $('tab-register').setAttribute('aria-selected', String(!isLogin));
  $('auth-submit').textContent = isLogin ? 'Anmelden' : 'Konto erstellen';
  $('auth-hint').hidden = isLogin;
  $('form-auth').elements.password.autocomplete = isLogin ? 'current-password' : 'new-password';
  clearError($('auth-error'));
}

$('tab-login').addEventListener('click', () => setMode('login'));
$('tab-register').addEventListener('click', () => setMode('register'));

$('form-auth').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const email = form.elements.email.value;
  const password = form.elements.password.value;
  clearError($('auth-error'));
  try {
    const user = await withBusy($('auth-submit'), () =>
      mode === 'login' ? api.login(email, password) : api.register(email, password),
    );
    form.reset();
    await offline.adopt(user); // Daten eines anderen Benutzers werden hier verworfen
    currentUser = user;
    articles = [];
    listLoaded = false;
    tagsStale = true;
    resetHash();
    route();
    savePendingShare();
  } catch (error) {
    showError($('auth-error'), error.message);
  }
});

$('btn-logout').addEventListener('click', async () => {
  try {
    await api.logout();
  } catch (error) {
    if (isUnreachable(error)) {
      // Ohne Verbindung ließe sich die Sitzung nicht beenden: dann lieber angemeldet bleiben.
      showError($('list-error'), 'Abmelden geht nur mit Internetverbindung, damit die Sitzung wirklich beendet wird.');
      return;
    }
    // Andere Fehler (z. B. Sitzung schon abgelaufen): lokal trotzdem abmelden.
  }
  endSession();
});

// ---------------------------------------------------------------------------
// Benutzermenü und Kontoseiten (#/account/password, #/account/delete)
// ---------------------------------------------------------------------------

function closeMenu() {
  $('user-menu').hidden = true;
  $('btn-user').setAttribute('aria-expanded', 'false');
}

$('btn-user').addEventListener('click', () => {
  const open = $('user-menu').hidden;
  $('user-menu').hidden = !open;
  $('btn-user').setAttribute('aria-expanded', String(open));
});

document.addEventListener('click', (event) => {
  if (!event.target.closest('.user-menu')) closeMenu();
});

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !$('user-menu').hidden) {
    closeMenu();
    $('btn-user').focus();
  }
});

/** Zeigt eine Kontoseite mit leerem Formular. */
function openAccountPage(name, title) {
  routeToken += 1;
  readerArticle = null;
  document.title = `${title} – ${APP_TITLE}`;
  const form = $(`form-${name}`);
  form.reset();
  clearError($(`${name}-error`));
  show(name);
  window.scrollTo(0, 0);
  form.elements[0].focus();
}

/** Zurück zur Artikelliste, ohne die Kontoseite im Verlauf zu behalten. */
function backToList(notice = '') {
  pendingNotice = notice;
  location.replace('#/');
}

// Export und Import (#/account/data)

const MAX_IMPORT_BYTES = 64 * 1024 * 1024; // wie im Backend

function openDataPage() {
  openAccountPage('data', 'Export und Import');
  $('data-result').hidden = true;
}

$('form-data').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  clearError($('data-error'));
  $('data-result').hidden = true;
  const file = form.elements.file.files[0];
  if (!file) {
    showError($('data-error'), 'Bitte eine Datei auswählen.');
    return;
  }
  if (file.size > MAX_IMPORT_BYTES) {
    showError($('data-error'), 'Die Datei ist größer als 64 MiB und kann nicht importiert werden.');
    return;
  }
  try {
    const result = await withBusy(form.querySelector('button[type="submit"]'), () =>
      api.importArticles(file),
    );
    form.reset();
    // Die Liste und die Schlagwörter müssen neu geladen werden.
    listLoaded = false;
    tagsStale = true;
    const parts = [`${result.imported} importiert`];
    parts.push(`${result.duplicates} schon vorhanden`);
    if (result.invalid > 0) parts.push(`${result.invalid} nicht übernommen`);
    if (result.highlights > 0) {
      parts.push(`${result.highlights} ${result.highlights === 1 ? 'Markierung' : 'Markierungen'}`);
    }
    if (result.highlights_invalid > 0) {
      parts.push(`${result.highlights_invalid} Markierungen nicht übernommen`);
    }
    $('data-result').textContent = `Fertig: ${parts.join(', ')}.`;
    $('data-result').hidden = false;
  } catch (error) {
    if (handleAuthError(error)) return;
    showError(
      $('data-error'),
      error instanceof ApiError && error.status === 413
        ? 'Die Datei ist zu groß.'
        : error.message,
    );
  }
});

// Share-Tokens (#/account/share)

function renderShareTokens(list) {
  const ul = $('share-list');
  ul.replaceChildren();
  $('share-empty').hidden = list.length > 0;
  for (const item of list) {
    const li = element('li', 'token-item');
    const info = element('div', 'token-info');
    info.append(element('strong', '', item.name));
    const used = item.last_used_at
      ? `zuletzt benutzt am ${formatDate(item.last_used_at)}`
      : 'noch nicht benutzt';
    info.append(element('span', 'hint', `angelegt am ${formatDate(item.created_at)}, ${used}`));
    const remove = element('button', 'danger-btn', 'Widerrufen');
    remove.type = 'button';
    remove.setAttribute('aria-label', `Token ${item.name} widerrufen`);
    remove.addEventListener('click', async () => {
      if (!window.confirm(`Token „${item.name}“ widerrufen? Das Gerät kann dann nichts mehr teilen.`)) return;
      try {
        await withBusy(remove, () => api.deleteShareToken(item.id));
        await loadShareTokens();
      } catch (error) {
        if (!handleAuthError(error)) showError($('share-error'), error.message);
      }
    });
    li.append(info, remove);
    ul.append(li);
  }
}

async function loadShareTokens() {
  const token = routeToken;
  try {
    const list = await api.listShareTokens();
    if (token === routeToken) renderShareTokens(list);
  } catch (error) {
    if (token === routeToken && !handleAuthError(error)) showError($('share-error'), error.message);
  }
}

function openSharePage() {
  openAccountPage('share', 'Teilen aus Apps');
  $('share-created').hidden = true;
  $('share-token-value').value = '';
  $('share-endpoint').textContent = `${location.origin}/api/share`;
  loadShareTokens();
}

$('form-share').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  clearError($('share-error'));
  try {
    const created = await withBusy(form.querySelector('button[type="submit"]'), () =>
      api.createShareToken(form.elements.name.value),
    );
    form.reset();
    $('share-token-value').value = created.token;
    $('share-created').hidden = false;
    $('share-token-value').select();
    await loadShareTokens();
  } catch (error) {
    if (!handleAuthError(error)) showError($('share-error'), error.message);
  }
});

$('share-copy').addEventListener('click', async () => {
  const input = $('share-token-value');
  input.select();
  try {
    await navigator.clipboard.writeText(input.value);
    $('share-copy').textContent = 'Kopiert';
  } catch {
    // Zwischenablage nicht verfügbar: der Text ist markiert und lässt sich manuell kopieren
  }
});

$('form-password').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  clearError($('password-error'));
  const { current, new: next, repeat } = form.elements;
  if (next.value !== repeat.value) {
    showError($('password-error'), 'Die neuen Passwörter stimmen nicht überein.');
    return;
  }
  try {
    await withBusy(form.querySelector('button[type="submit"]'), () =>
      api.changePassword(current.value, next.value),
    );
    form.reset();
    backToList('Das Passwort wurde geändert.');
  } catch (error) {
    if (!handleAuthError(error)) showError($('password-error'), error.message);
  }
});

$('form-delete').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  clearError($('delete-error'));
  if (!window.confirm('Konto und alle Artikel wirklich unwiderruflich löschen?')) return;
  try {
    await withBusy(form.querySelector('button[type="submit"]'), () =>
      api.deleteAccount(form.elements.password.value),
    );
    form.reset();
    endSession();
  } catch (error) {
    if (!handleAuthError(error)) showError($('delete-error'), error.message);
  }
});

// ---------------------------------------------------------------------------
// Navigation (Adressleiste: "#/article/<id>" öffnet den Lesemodus,
// "#/account/password" und "#/account/delete" die Kontoseiten)
// ---------------------------------------------------------------------------

function route() {
  if (!currentUser) {
    show('auth');
    return;
  }
  closeMenu();
  const match = /^#\/article\/(\d+)$/.exec(location.hash);
  if (match) openReader(Number(match[1]));
  else if (location.hash === '#/account/share') openSharePage();
  else if (location.hash === '#/account/data') openDataPage();
  else if (location.hash === '#/account/display') openDisplayPage();
  else if (location.hash === '#/account/offline') openOfflinePage();
  else if (location.hash === '#/account/password') openAccountPage('password', 'Passwort ändern');
  else if (location.hash === '#/account/delete') openAccountPage('delete', 'Konto löschen');
  else openList();
}

window.addEventListener('hashchange', route);

// ---------------------------------------------------------------------------
// Liste
// ---------------------------------------------------------------------------

function openList() {
  routeToken += 1;
  readerArticle = null;
  document.title = APP_TITLE;
  $('user-email').textContent = currentUser.email;
  $('filter-q').value = filter.q;
  $('filter-read').value = filter.read;
  const notice = $('home-notice');
  notice.textContent = pendingNotice;
  notice.hidden = !pendingNotice;
  pendingNotice = '';
  show('home');
  if (tagsStale) loadTags();
  else renderTags();
  if (!listLoaded) loadArticles(true);
  else renderList();
}

function filterActive() {
  return Boolean(filter.q || filter.read || filter.tag);
}

async function loadArticles(reset) {
  clearError($('list-error'));
  const token = ++listToken;
  const offset = reset ? 0 : articles.length;
  // Bei schlechter Verbindung nicht ewig leer bleiben: nach kurzer Zeit erscheinen die
  // gespeicherten Artikel, die Serverliste ersetzt sie, sobald sie da ist.
  const slow = reset && articles.length === 0 ? setTimeout(() => showStoredList(token), SLOW_MS) : 0;
  try {
    const page = await api.listArticles(PAGE_SIZE, offset, filter);
    clearTimeout(slow);
    if (token !== listToken) return; // Filter wurde inzwischen geändert
    setOfflineBanner(false);
    articles = reset ? page : articles.concat(page);
    hasMore = page.length === PAGE_SIZE;
    listLoaded = true;
    renderList();
    if (reset) scheduleSync();
  } catch (error) {
    clearTimeout(slow);
    if (token !== listToken) return;
    if (handleAuthError(error)) return;
    if (isUnreachable(error) && (await showStoredList(token))) return;
    showError($('list-error'), error.message);
  }
}

/** Zeigt die gespeicherten Artikel (mit dem aktuellen Filter). Liefert false, wenn es keine gibt. */
async function showStoredList(token) {
  if ((await offline.count()) === 0) return false;
  const stored = await offline.list(filter);
  if (token !== listToken || !currentUser) return true;
  clearError($('list-error'));
  setOfflineBanner(true);
  articles = stored;
  hasMore = false;
  listLoaded = false; // beim nächsten Besuch wieder beim Server nachfragen
  renderList();
  if (tagsStale || tags.length === 0) {
    tags = await offline.tagCounts();
    renderTags();
  }
  return true;
}

/** Gleicht den Offline-Speicher im Hintergrund ab (höchstens alle fünf Minuten). */
function scheduleSync(force = false) {
  if (!currentUser || (!force && Date.now() - lastSync < 5 * 60 * 1000)) return Promise.resolve(null);
  lastSync = Date.now();
  return offline
    .sync(api)
    .then((result) => {
      if (result.offline) lastSync = 0; // beim nächsten Mal erneut versuchen
      return result;
    })
    .catch((error) => {
      handleAuthError(error);
      return null;
    });
}

async function loadTags() {
  try {
    tags = await api.listTags();
    tagsStale = false;
    // Ein Schlagwort, das es nicht mehr gibt, bleibt nicht als Filter hängen.
    if (filter.tag && !tags.some((t) => t.name.toLowerCase() === filter.tag.toLowerCase())) {
      filter.tag = '';
      loadArticles(true);
    }
    renderTags();
  } catch (error) {
    if (handleAuthError(error)) return;
    if (isUnreachable(error)) {
      tags = await offline.tagCounts(); // ohne Verbindung: Schlagwörter der gespeicherten Artikel
      renderTags();
    }
    // andere Fehler: die Liste funktioniert auch ohne Leiste
  }
}

function renderTags() {
  const bar = $('tag-bar');
  bar.hidden = tags.length === 0;
  bar.replaceChildren(
    ...tags.map(({ name, count }) => {
      const chip = element('button', 'tag-chip', name);
      chip.type = 'button';
      chip.append(element('span', 'tag-count', String(count)));
      const active = name.toLowerCase() === filter.tag.toLowerCase();
      chip.setAttribute('aria-pressed', String(active));
      chip.addEventListener('click', () => {
        filter.tag = active ? '' : name;
        applyFilter();
      });
      return chip;
    }),
  );
}

/** Lädt die Liste mit dem aktuellen Filter neu. */
function applyFilter() {
  renderTags();
  articles = [];
  hasMore = false;
  listLoaded = false;
  renderList();
  loadArticles(true);
}

let searchTimer = 0;
$('form-filter').addEventListener('submit', (event) => {
  event.preventDefault();
  clearTimeout(searchTimer);
  filter.q = $('filter-q').value.trim();
  applyFilter();
});
$('filter-q').addEventListener('input', () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    const q = $('filter-q').value.trim();
    if (q === filter.q) return;
    filter.q = q;
    applyFilter();
  }, 300);
});
$('filter-read').addEventListener('change', () => {
  filter.read = $('filter-read').value;
  applyFilter();
});

function renderList() {
  $('article-list').replaceChildren(...articles.map(articleItem));
  const empty = $('list-empty');
  empty.textContent = offlineView
    ? 'Keine passenden gespeicherten Artikel.'
    : filterActive()
      ? 'Keine passenden Artikel gefunden.'
      : 'Noch keine Artikel. Füge oben eine Adresse ein.';
  empty.hidden = articles.length > 0 || (!listLoaded && !offlineView);
  $('btn-more').hidden = !hasMore;
}

/** Suchauszug anzeigen: Treffer stehen zwischen \u0001 und \u0002 und werden als <mark> gesetzt. */
function snippetNodes(snippet) {
  const nodes = [];
  snippet.split('\u0001').forEach((part, index) => {
    if (index === 0) {
      nodes.push(part);
      return;
    }
    const [hit, rest = ''] = part.split('\u0002');
    nodes.push(element('mark', '', hit), rest);
  });
  return nodes;
}

function articleItem(article) {
  const item = element('li', article.is_read ? 'article is-read' : 'article');

  const excerpt = element('span', 'article-excerpt');
  if (article.snippet) excerpt.append(...snippetNodes(article.snippet));
  else excerpt.textContent = article.excerpt;

  const link = element('a', 'article-main');
  link.href = `#/article/${article.id}`;
  link.append(
    element('span', 'article-title', article.title),
    element('span', 'article-meta', `${hostOf(article.url)} · ${formatDate(article.created_at)}`),
    excerpt,
  );
  if (article.tags.length > 0) {
    link.append(element('span', 'article-tags', article.tags.map((t) => `#${t}`).join(' ')));
  }

  const toggle = element('button', 'icon-btn toggle', article.is_read ? 'Ungelesen' : 'Gelesen');
  toggle.type = 'button';
  toggle.setAttribute(
    'aria-label',
    `${article.is_read ? 'Als ungelesen' : 'Als gelesen'} markieren: ${article.title}`,
  );
  toggle.addEventListener('click', () => toggleRead(article, toggle));

  const remove = element('button', 'icon-btn delete', 'Löschen');
  remove.type = 'button';
  remove.setAttribute('aria-label', `Artikel löschen: ${article.title}`);
  remove.addEventListener('click', async () => {
    if (await removeArticle(article.id, article.title)) renderList();
  });

  const actions = element('div', 'article-actions');
  actions.append(toggle, remove);
  item.append(link, actions);
  return item;
}

/** Ändert den Gelesen-Status und übernimmt die Antwort in die geladene Liste. */
async function setRead(article, isRead) {
  const updated = await api.updateArticle(article.id, { is_read: isRead });
  mergeUpdated(updated);
  return updated;
}

function mergeUpdated(updated) {
  // Gelesene Artikel starten oben (auch das Backend setzt die Position dann zurück). Die
  // gespeicherte Kopie soll das ebenfalls wissen, sonst springt sie beim nächsten Öffnen kurz.
  if (updated.is_read === true) offline.setPosition(updated.id, 0);
  const index = articles.findIndex((a) => a.id === updated.id);
  if (index === -1) return;
  const wanted = filter.read === '' ? null : filter.read === 'true';
  if (wanted !== null && updated.is_read !== wanted) {
    articles.splice(index, 1); // passt nicht mehr zum Filter
  } else {
    articles[index] = { ...articles[index], ...updated, snippet: articles[index].snippet };
  }
}

async function toggleRead(article, button) {
  try {
    await withBusy(button, () => setRead(article, !article.is_read));
    renderList();
  } catch (error) {
    if (handleAuthError(error)) return;
    if (isUnreachable(error)) {
      // Ohne Verbindung vormerken, der Abgleich sendet es später.
      const isRead = !article.is_read;
      await offline.queueRead(article.id, isRead);
      mergeUpdated({ ...article, is_read: isRead });
      renderList();
      showNotice('Ohne Verbindung vorgemerkt, wird später übertragen.');
      return;
    }
    if (error instanceof ApiError && error.status === 404) {
      articles = articles.filter((a) => a.id !== article.id);
      renderList();
      return;
    }
    showError($('list-error'), error.message);
  }
}

/** Fragt nach, löscht den Artikel und entfernt ihn aus der Liste. Gibt zurück, ob er weg ist. */
async function removeArticle(id, title) {
  if (!window.confirm(`„${title}“ wirklich löschen?`)) return false;
  try {
    await api.deleteArticle(id);
  } catch (error) {
    // 404: war schon weg, das Ziel ist erreicht.
    if (!(error instanceof ApiError && error.status === 404)) {
      if (!handleAuthError(error)) showError($('list-error'), error.message);
      return false;
    }
  }
  articles = articles.filter((a) => a.id !== id);
  tagsStale = true;
  if (!$('view-home').hidden) loadTags();
  return true;
}

$('form-add').addEventListener('submit', async (event) => {
  event.preventDefault();
  const input = $('add-url');
  const url = input.value.trim();
  if (!url) return;

  clearError($('add-error'));
  $('add-status').textContent = 'Seite wird geladen … das kann einige Sekunden dauern.';
  $('add-status').hidden = false;
  input.readOnly = true;
  try {
    const article = await withBusy($('add-submit'), () => api.addArticle(url));
    input.value = '';
    if (filterActive()) {
      // Der neue Artikel passt vielleicht nicht zum Filter: Filter zurücksetzen.
      filter = { q: '', read: '', tag: '' };
      $('filter-q').value = '';
      $('filter-read').value = '';
      applyFilter();
    } else {
      articles.unshift(article);
      renderList();
    }
  } catch (error) {
    if (!handleAuthError(error)) showError($('add-error'), error.message);
  } finally {
    input.readOnly = false;
    $('add-status').hidden = true;
  }
});

$('btn-more').addEventListener('click', () => withBusy($('btn-more'), () => loadArticles(false)));

// ---------------------------------------------------------------------------
// Lesemodus
// ---------------------------------------------------------------------------

/** Setzt die Sprache des Artikels (Silbentrennung, Screenreader); ohne Angabe gilt die der App. */
function setArticleLang(lang) {
  for (const element of [$('reader-title'), $('reader-content')]) {
    if (lang) element.lang = lang;
    else element.removeAttribute('lang');
  }
}

function clearReaderView() {
  setArticleLang('');
  readerArticle = null;
  $('reader-title').textContent = '';
  $('reader-meta').replaceChildren();
  $('reader-content').replaceChildren();
  highlighter.reset();
  $('reader-delete').hidden = true;
  $('reader-refresh-images').hidden = true;
  $('reader-toggle-read').hidden = true;
  $('form-tags').hidden = true;
  clearError($('reader-error'));
  clearError($('tags-error'));
  $('reader-notice').hidden = true;
}

/** Baut den Lesemodus für einen Artikel auf (aus dem Server oder dem Offline-Speicher). */
function renderArticle(article) {
  readerArticle = article;
  document.title = `${article.title} – ${APP_TITLE}`;
  $('reader-title').textContent = article.title;

  const meta = $('reader-meta');
  meta.replaceChildren();
  const href = safeUrl(article.url, location.href, ['http:', 'https:']);
  if (href) {
    const source = element('a', '', hostOf(article.url));
    source.href = href;
    source.target = '_blank';
    source.rel = 'noopener noreferrer';
    meta.append(source);
  } else {
    meta.append(hostOf(article.url));
  }
  meta.append(` · gespeichert am ${formatDate(article.created_at)}`);

  renderSafeHtml($('reader-content'), article.content, article.url, article.images);
  updateImageRefresh(article);
  setArticleLang(article.lang);
  $('reader-delete').hidden = false;
  updateReaderControls();
  $('form-tags').hidden = false;
  highlighter.open(article.id, article.highlights);
  position.begin(article); // springt zur gemerkten Stelle und merkt sich die weiteren
  backBubble.schedule(); // der Text ist da: Seitenende und Link unten haben sich verschoben
}

/**
 * Merkt die Leseposition still im Hintergrund: Der Benutzer soll davon nichts mitbekommen, also
 * keine Meldung bei Fehlern. Ohne Verbindung wird sie vorgemerkt und beim Abgleich gesendet;
 * alles andere (z. B. zu viele Anfragen) erledigt sich mit der nächsten Position.
 */
async function saveReadingPosition(id, pos, options) {
  offline.setPosition(id, pos); // die gespeicherte Kopie soll auch ohne Netz stimmen
  try {
    await api.updateArticle(id, { read_pos: pos }, options);
    offline.dropQueuedPosition(id);
  } catch (error) {
    if (isUnreachable(error)) offline.queueChange(id, { read_pos: pos });
    else handleAuthError(error);
  }
}

/**
 * Tauscht inzwischen lokal vorliegende Bilder ein und zeigt „Bilder nachladen“ nur dann, wenn
 * noch Bilder von der Originaladresse kommen (bei älteren, importierten oder gescheiterten).
 */
function updateImageRefresh(article) {
  const remote = applyLocalImages($('reader-content'), article.images);
  $('reader-refresh-images').hidden = remote === 0;
  return remote;
}

async function openReader(id) {
  position.end(); // Position des vorigen Artikels sichern, bevor sein Text verschwindet
  const token = ++routeToken;
  show('reader');
  window.scrollTo(0, 0);
  clearReaderView();

  // Gespeicherte Kopie sofort anzeigen (auch bei schlechter Verbindung), danach beim Server
  // nachsehen, ob sich Status, Schlagwörter oder Markierungen geändert haben.
  const stored = await offline.getArticle(id);
  if (token !== routeToken) return;
  if (stored) renderArticle(stored);

  try {
    const article = await api.getArticle(id);
    if (token !== routeToken) return;
    setOfflineBanner(false);
    offline.putArticle(article, { opened: true });
    if (!stored || stored.content !== article.content) {
      clearReaderView();
      renderArticle(article);
    } else if (!isEditingTags()) {
      readerArticle = article;
      updateReaderControls();
      highlighter.open(article.id, article.highlights);
      // Der Text bleibt, aber vielleicht liegen Bilder inzwischen lokal vor.
      updateImageRefresh(article);
    }
    // Der Server kennt vielleicht eine neuere Stelle (auf einem anderen Gerät weitergelesen).
    if (readerArticle?.id === article.id) position.adopt(article);
  } catch (error) {
    if (token !== routeToken) return;
    if (handleAuthError(error)) return;
    if (isUnreachable(error)) {
      if (stored) setOfflineBanner(true);
      else showError($('reader-error'), 'Dieser Artikel ist auf diesem Gerät nicht gespeichert. Ohne Internetverbindung lässt er sich nicht öffnen.');
      return;
    }
    if (error instanceof ApiError && error.status === 404) offline.remove(id);
    clearReaderView();
    showError($('reader-error'), error.message);
  }
}

function isEditingTags() {
  return document.activeElement === $('tags-input');
}

function updateReaderControls() {
  const button = $('reader-toggle-read');
  button.textContent = readerArticle.is_read ? 'Als ungelesen markieren' : 'Als gelesen markieren';
  button.hidden = false;
  $('tags-input').value = readerArticle.tags.join(', ');
}

/** Übernimmt die Antwort des Backends für den geöffneten Artikel. */
function applyReaderUpdate(updated) {
  readerArticle = { ...readerArticle, is_read: updated.is_read, tags: updated.tags };
  position.setEnabled(!updated.is_read); // gelesen: oben beginnen, nichts mehr merken
  mergeUpdated(updated);
  tagsStale = true;
  if (filterActive()) listLoaded = false; // Liste beim Zurückgehen neu laden
  updateReaderControls();
}

async function patchReader(changes, button, errorElement) {
  if (!readerArticle) return;
  const token = routeToken;
  clearError(errorElement);
  $('reader-notice').hidden = true;
  try {
    const updated = await withBusy(button, () => api.updateArticle(readerArticle.id, changes));
    if (token !== routeToken) return;
    applyReaderUpdate(updated);
  } catch (error) {
    if (token !== routeToken || handleAuthError(error)) return;
    if (isUnreachable(error) && Object.keys(changes).length === 1 && 'is_read' in changes) {
      // Ohne Verbindung vormerken, der Abgleich sendet es später.
      await offline.queueRead(readerArticle.id, changes.is_read);
      readerArticle = { ...readerArticle, is_read: changes.is_read };
      position.setEnabled(!changes.is_read);
      mergeUpdated({ id: readerArticle.id, is_read: changes.is_read });
      listLoaded = false;
      updateReaderControls();
      $('reader-notice').textContent = 'Ohne Verbindung vorgemerkt, wird später übertragen.';
      $('reader-notice').hidden = false;
      return;
    }
    showError(errorElement, error.message);
  }
}

$('reader-toggle-read').addEventListener('click', () => {
  if (readerArticle) {
    patchReader({ is_read: !readerArticle.is_read }, $('reader-toggle-read'), $('tags-error'));
  }
});

$('reader-refresh-images').addEventListener('click', async () => {
  if (!readerArticle) return;
  const { id } = readerArticle;
  const token = routeToken;
  const button = $('reader-refresh-images');
  clearError($('reader-error'));
  try {
    await withBusy(button, () => api.refreshImages(id));
    $('reader-notice').textContent = 'Die Bilder werden im Hintergrund geladen …';
    $('reader-notice').hidden = false;
    // Der Server arbeitet im Hintergrund: ein paar Mal nachsehen, solange der Artikel offen ist.
    let remote = 1;
    for (let attempt = 0; attempt < 6 && remote > 0; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 3000));
      if (token !== routeToken) return;
      const article = await api.getArticle(id);
      if (token !== routeToken) return;
      readerArticle = { ...readerArticle, images: article.images };
      offline.putArticle(article, { opened: true });
      remote = updateImageRefresh(article);
    }
    $('reader-notice').textContent =
      remote === 0
        ? 'Alle Bilder liegen jetzt auf dem Server.'
        : 'Nicht alle Bilder ließen sich laden. Später kann man es noch einmal versuchen.';
  } catch (error) {
    if (token !== routeToken || handleAuthError(error)) return;
    $('reader-notice').hidden = true;
    showError($('reader-error'), error.message);
  }
});

$('form-tags').addEventListener('submit', (event) => {
  event.preventDefault();
  const names = $('tags-input').value.split(',').map((t) => t.trim()).filter(Boolean);
  patchReader({ tags: names }, $('tags-submit'), $('tags-error'));
});

$('reader-delete').addEventListener('click', async () => {
  if (!readerArticle) return;
  if (await removeArticle(readerArticle.id, readerArticle.title)) {
    position.cancel(); // den gelöschten Artikel braucht keine Position mehr
    resetHash();
    route();
  }
});

// ---------------------------------------------------------------------------
// Darstellung (#/account/display) und Bedienfeld „Aa“ im Lesemodus (js/reading.js)
// ---------------------------------------------------------------------------

mountControls($('display-controls'));
mountControls($('reading-panel'));

function setReadingPanel(open) {
  $('reading-panel').hidden = !open;
  $('reading-toggle').setAttribute('aria-expanded', String(open));
}

$('reading-toggle').addEventListener('click', () => {
  setReadingPanel($('reading-panel').hidden);
});

document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape' || $('reading-panel').hidden) return;
  setReadingPanel(false);
  $('reading-toggle').focus();
});

function openDisplayPage() {
  routeToken += 1;
  readerArticle = null;
  document.title = `Darstellung – ${APP_TITLE}`;
  show('display');
  window.scrollTo(0, 0);
  $('display-controls').querySelector('select, input').focus();
}

// ---------------------------------------------------------------------------
// Offline-Lesen (#/account/offline)
// ---------------------------------------------------------------------------

async function showOfflineStatus() {
  const { count, bytes } = await offline.stats();
  const form = $('form-offline');
  form.elements.enabled.checked = offline.isEnabled();
  $('offline-sync').disabled = !offline.isEnabled();
  $('offline-clear').disabled = count === 0;
  if (!offline.isEnabled()) {
    $('offline-status').textContent = 'Das Speichern für das Lesen ohne Internet ist ausgeschaltet.';
  } else if (count === 0) {
    $('offline-status').textContent = 'Auf diesem Gerät ist noch nichts gespeichert.';
  } else {
    const mb = Math.max(0.1, bytes / (1024 * 1024)).toFixed(1).replace('.', ',');
    $('offline-status').textContent = `Auf diesem Gerät gespeichert: ${count} Artikel (etwa ${mb} MB).`;
  }
}

function openOfflinePage() {
  openAccountPage('offline', 'Offline-Lesen');
  showOfflineStatus();
}

$('form-offline').elements.enabled.addEventListener('change', async (event) => {
  await offline.setEnabled(event.target.checked);
  if (event.target.checked) await runOfflineSync();
  else await showOfflineStatus();
});

async function runOfflineSync() {
  clearError($('offline-error'));
  $('offline-status').textContent = 'Wird aktualisiert …';
  const result = await scheduleSync(true);
  if (result?.offline) showError($('offline-error'), 'Keine Verbindung zum Server. Es konnte nichts aktualisiert werden.');
  await showOfflineStatus();
}

$('form-offline').addEventListener('submit', (event) => {
  event.preventDefault();
  withBusy($('offline-sync'), runOfflineSync);
});

$('offline-clear').addEventListener('click', async () => {
  await offline.clearAll();
  if (currentUser) await offline.adopt(currentUser);
  await showOfflineStatus();
});

// ---------------------------------------------------------------------------
// Verbindung, Teilen-Ziel, Service Worker
// ---------------------------------------------------------------------------

/** Wieder online (oder „Erneut versuchen“): Liste oder Artikel neu laden, Speicher abgleichen. */
function retryConnection() {
  if (!currentUser) return;
  scheduleSync(true);
  if (!$('view-home').hidden) loadArticles(true);
  else if (!$('view-reader').hidden) route();
}

$('offline-retry').addEventListener('click', retryConnection);
window.addEventListener('online', () => {
  if (offlineView) retryConnection();
  else scheduleSync();
});

/** Über „Teilen“ erhaltene Adresse speichern, sobald der Benutzer angemeldet ist. */
async function savePendingShare() {
  const url = pendingShare;
  pendingShare = '';
  if (!url || !currentUser) return;
  clearError($('list-error'));
  showNotice('Artikel wird gespeichert …');
  try {
    const article = await api.addArticle(url);
    listLoaded = false;
    tagsStale = true;
    showNotice(`Gespeichert: ${article.title}`);
    if (!$('view-home').hidden) loadArticles(true);
  } catch (error) {
    if (handleAuthError(error)) return;
    showNotice('');
    showError($('list-error'), error.message);
  }
}

if ('serviceWorker' in navigator) {
  // Fehler (z. B. unsicherer Kontext ohne HTTPS) sind harmlos: die App läuft auch ohne.
  navigator.serviceWorker.register('/sw.js').catch(() => {});
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function init() {
  // Bei schlechter Verbindung nicht auf den Server warten: mit gespeicherten Daten starten
  // und die Anmeldung im Hintergrund prüfen.
  const owner = offline.isEnabled() ? await offline.getOwner() : null;
  let routed = false;
  const startStored = () => {
    currentUser = owner;
    setOfflineBanner(true);
    routed = true;
    route();
  };
  const slow = owner ? setTimeout(startStored, BOOT_SLOW_MS) : 0;

  try {
    const user = await api.me();
    clearTimeout(slow);
    await offline.adopt(user);
    currentUser = user;
    setOfflineBanner(false);
    if (routed) {
      // Schon mit gespeicherten Daten gestartet: nicht aus dem Lesen reißen, nur die Liste auffrischen.
      if (!$('view-home').hidden) loadArticles(true);
    } else {
      route();
    }
    savePendingShare();
  } catch (error) {
    clearTimeout(slow);
    if (isUnreachable(error) && owner) {
      if (!routed) startStored();
      return;
    }
    if (handleAuthError(error)) return;
    show('auth'); // Server nicht erreichbar oder anderer Fehler: gespeicherte Daten bleiben
    if (!(error instanceof ApiError && error.status === 401)) {
      showError($('auth-error'), error.message);
    }
  }
}

init();
