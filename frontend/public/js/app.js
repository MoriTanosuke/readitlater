import { api, ApiError } from './api.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['loading', 'auth', 'home'];

let mode = 'login'; // 'login' | 'register'

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

function showHome(user) {
  $('user-email').textContent = user.email; // textContent: kein HTML-Einschleusen
  show('home');
}

async function withBusy(button, task) {
  button.disabled = true;
  try {
    return await task();
  } finally {
    button.disabled = false;
  }
}

async function init() {
  try {
    showHome(await api.me());
  } catch (error) {
    show('auth');
    if (!(error instanceof ApiError) || error.status !== 401) {
      showError($('auth-error'), error.message);
    }
  }
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
    showHome(user);
  } catch (error) {
    showError($('auth-error'), error.message);
  }
});

$('btn-logout').addEventListener('click', async () => {
  try {
    await api.logout();
  } finally {
    setMode('login');
    show('auth');
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
    setMode('login');
    show('auth');
  } catch (error) {
    showError($('delete-error'), error.message);
  }
});

init();
