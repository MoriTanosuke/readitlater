import { api, ApiError } from './api.js';
import { renderSafeHtml, safeUrl } from './safe-html.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['loading', 'auth', 'home', 'reader'];
const PAGE_SIZE = 50;
const APP_TITLE = 'Leseliste';

let mode = 'login'; // 'login' | 'register'
let currentUser = null;
let articles = []; // bereits geladene Artikel der Liste
let hasMore = false;
let listLoaded = false;
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

$('form-password').addEventListener('submit', async (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  clearError($('password-error'));
  $('password-ok').hidden = true;
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
    $('password-ok').textContent = 'Das Passwort wurde geändert.';
    $('password-ok').hidden = false;
  } catch (error) {
    showError($('password-error'), error.message);
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
    form.closest('details').open = false;
    endSession();
  } catch (error) {
    showError($('delete-error'), error.message);
  }
});

// ---------------------------------------------------------------------------
// Navigation (Adressleiste: "#/article/<id>" öffnet den Lesemodus)
// ---------------------------------------------------------------------------

function route() {
  if (!currentUser) {
    show('auth');
    return;
  }
  const match = /^#\/article\/(\d+)$/.exec(location.hash);
  if (match) openReader(Number(match[1]));
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
  show('home');
  if (!listLoaded) loadArticles(true);
  else renderList();
}

async function loadArticles(reset) {
  clearError($('list-error'));
  const offset = reset ? 0 : articles.length;
  try {
    const page = await api.listArticles(PAGE_SIZE, offset);
    articles = reset ? page : articles.concat(page);
    hasMore = page.length === PAGE_SIZE;
    listLoaded = true;
    renderList();
  } catch (error) {
    if (handleAuthError(error)) return;
    showError($('list-error'), error.message);
  }
}

function renderList() {
  $('article-list').replaceChildren(...articles.map(articleItem));
  $('list-empty').hidden = articles.length > 0 || !listLoaded;
  $('btn-more').hidden = !hasMore;
}

function articleItem(article) {
  const item = element('li', 'article');

  const link = element('a', 'article-main');
  link.href = `#/article/${article.id}`;
  link.append(
    element('span', 'article-title', article.title),
    element('span', 'article-meta', `${hostOf(article.url)} · ${formatDate(article.created_at)}`),
    element('span', 'article-excerpt', article.excerpt),
  );

  const remove = element('button', 'icon-btn', 'Löschen');
  remove.type = 'button';
  remove.setAttribute('aria-label', `Artikel löschen: ${article.title}`);
  remove.addEventListener('click', async () => {
    if (await removeArticle(article.id, article.title)) renderList();
  });

  item.append(link, remove);
  return item;
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
    articles.unshift(article);
    input.value = '';
    renderList();
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
  clearError($('reader-error'));

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
  } catch (error) {
    if (token !== routeToken) return;
    if (handleAuthError(error)) return;
    showError($('reader-error'), error.message);
  }
}

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
