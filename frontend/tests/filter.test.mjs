// Die Auswahl Alle/Ungelesen/Gelesen bleibt erhalten (pro Gerät in localStorage).
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL, KAFFEE } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/zweiter.html': KAFFEE,
});
after(teardown);

const KEY = 'leseliste-read-filter';
const items = () => [...document.querySelectorAll('#article-list li')];
const choose = (value) => {
  $('filter-read').value = value;
  $('filter-read').dispatchEvent(new window.Event('change'));
};

test('Lesefilter bleibt nach dem Speichern und nach erneuter Anmeldung', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const form = $('form-auth');
  form.elements.email.value = 'filter@example.org';
  form.elements.password.value = 'passwort-12345';
  form.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  $('add-url').value = site.url('/artikel.html');
  $('form-add').requestSubmit();
  await until(() => items().length === 1, 'erster Artikel');
  items()[0].querySelector('.toggle').click();
  await until(() => items()[0].classList.contains('is-read'), 'als gelesen markiert');

  choose('false');
  await until(() => items().length === 0, 'Ungelesen: leer');
  assert.equal(window.localStorage.getItem(KEY), 'false');

  // Neuer Artikel: Auswahl bleibt, der ungelesene Artikel passt zum Filter.
  $('add-url').value = site.url('/zweiter.html');
  $('form-add').requestSubmit();
  await until(() => items().length === 1, 'neuer Artikel sichtbar');
  assert.equal($('filter-read').value, 'false', 'Auswahl bleibt nach dem Speichern');
  assert.equal(items().length, 1, 'gelesener Artikel bleibt ausgeblendet');
  ok('Auswahl Ungelesen bleibt nach dem Speichern');

  // Suche zurücknehmen, dann „Gelesen“ wählen.
  $('filter-q').value = 'kaffee';
  $('form-filter').requestSubmit();
  await until(() => items().length === 1, 'Suche');
  $('filter-q').value = '';
  $('form-filter').requestSubmit();
  choose('true');
  assert.equal(window.localStorage.getItem(KEY), 'true');
  await until(() => items().length === 1 && items()[0].classList.contains('is-read'), 'Gelesen');

  // Abmelden und wieder anmelden: dieselbe Auswahl.
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'abgemeldet');
  const login = $('form-auth');
  $('tab-login').click();
  login.elements.email.value = 'filter@example.org';
  login.elements.password.value = 'passwort-12345';
  login.requestSubmit();
  await until(() => visible('view-home'), 'wieder angemeldet');
  assert.equal($('filter-read').value, 'true');
  await until(() => items().length === 1 && items()[0].classList.contains('is-read'), 'Liste gefiltert');

  choose('');
  assert.equal(window.localStorage.getItem(KEY), null, 'Alle löscht den Eintrag');
  ok('Auswahl überlebt Abmelden und Anmelden, „Alle“ räumt auf');
});
