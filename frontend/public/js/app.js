import { api, ApiError } from './api.js';
import { renderSafeHtml, safeUrl } from './safe-html.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['loading', 'auth', 'home', 'reader', 'password', 'delete', 'share'];
const PAGE_SIZE = 50;
const APP_TITLE = 'Leseliste';

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

// ---------------------------------------------------------------------------
// Hilfsfunktionen
// ---------------------------------------------------------------------------

function show(view) {
  for (const name of VIEWS) $(`view-${name}`).hidden = name !== view;
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
    currentUser = user;
    articles = [];
    listLoaded = false;
    tagsStale = true;
    resetHash();
    route();
  } catch (error) {
    showError($('auth-error'), error.message);
  }
});

$('btn-logout').addEventListener('click', async () => {
  try {
    await api.logout();
  } finally {
    endSession();
  }
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
  try {
    const page = await api.listArticles(PAGE_SIZE, offset, filter);
    if (token !== listToken) return; // Filter wurde inzwischen geändert
    articles = reset ? page : articles.concat(page);
    hasMore = page.length === PAGE_SIZE;
    listLoaded = true;
    renderList();
  } catch (error) {
    if (token !== listToken) return;
    if (handleAuthError(error)) return;
    showError($('list-error'), error.message);
  }
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
    handleAuthError(error); // andere Fehler: die Liste funktioniert auch ohne Leiste
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
  empty.textContent = filterActive()
    ? 'Keine passenden Artikel gefunden.'
    : 'Noch keine Artikel. Füge oben eine Adresse ein.';
  empty.hidden = articles.length > 0 || !listLoaded;
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

async function openReader(id) {
  const token = ++routeToken;
  readerArticle = null;
  show('reader');
  window.scrollTo(0, 0);
  $('reader-title').textContent = '';
  $('reader-meta').replaceChildren();
  $('reader-content').replaceChildren();
  $('reader-delete').hidden = true;
  $('reader-toggle-read').hidden = true;
  $('form-tags').hidden = true;
  clearError($('reader-error'));
  clearError($('tags-error'));

  try {
    const article = await api.getArticle(id);
    if (token !== routeToken) return; // inzwischen woanders hin navigiert
    readerArticle = article;
    document.title = `${article.title} – ${APP_TITLE}`;
    $('reader-title').textContent = article.title;

    const meta = $('reader-meta');
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

    renderSafeHtml($('reader-content'), article.content, article.url);
    $('reader-delete').hidden = false;
    updateReaderControls();
    $('form-tags').hidden = false;
  } catch (error) {
    if (token !== routeToken) return;
    if (handleAuthError(error)) return;
    showError($('reader-error'), error.message);
  }
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
  mergeUpdated(updated);
  tagsStale = true;
  if (filterActive()) listLoaded = false; // Liste beim Zurückgehen neu laden
  updateReaderControls();
}

async function patchReader(changes, button, errorElement) {
  if (!readerArticle) return;
  const token = routeToken;
  clearError(errorElement);
  try {
    const updated = await withBusy(button, () => api.updateArticle(readerArticle.id, changes));
    if (token !== routeToken) return;
    applyReaderUpdate(updated);
  } catch (error) {
    if (token !== routeToken || handleAuthError(error)) return;
    showError(errorElement, error.message);
  }
}

$('reader-toggle-read').addEventListener('click', () => {
  if (readerArticle) {
    patchReader({ is_read: !readerArticle.is_read }, $('reader-toggle-read'), $('tags-error'));
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
    resetHash();
    route();
  }
});

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function init() {
  try {
    currentUser = await api.me();
    route();
  } catch (error) {
    show('auth');
    if (!(error instanceof ApiError) || error.status !== 401) {
      showError($('auth-error'), error.message);
    }
  }
}

init();
