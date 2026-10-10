// Frontend-Ablauf: Benutzermenü, Kontoseiten mit Abbrechen, Passwort ändern, Konto löschen.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL, KAFFEE } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/zweiter.html': KAFFEE,
});
after(teardown);

test('Benutzermenü, Passwort ändern und Konto löschen', async (t) => {
  const ok = steps(t);

  const click = (el) => el.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true }));

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const auth = $('form-auth');
  auth.elements.email.value = 'konto@example.org';
  auth.elements.password.value = 'passwort-12345';
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  // Die Kontoformulare sind nicht mehr Teil der Liste
  assert.equal($('view-home').querySelector('#form-password, #form-delete, details'), null);
  ok('Artikelliste enthält keine Kontoformulare mehr');

  // Menü
  assert.equal($('user-menu').hidden, true);
  assert.equal($('btn-user').textContent.includes('konto@example.org'), true);
  click($('btn-user'));
  assert.equal($('user-menu').hidden, false);
  assert.equal($('btn-user').getAttribute('aria-expanded'), 'true');
  assert.deepEqual(
    [...$('user-menu').querySelectorAll('a')].map((a) => [a.textContent, a.getAttribute('href')]),
    [
      ['Teilen aus Apps', '#/account/share'],
      ['Passwort ändern', '#/account/password'],
      ['Konto löschen', '#/account/delete'],
    ],
  );
  document.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  assert.equal($('user-menu').hidden, true);
  assert.equal($('btn-user').getAttribute('aria-expanded'), 'false');
  click($('btn-user'));
  click($('add-url'));
  assert.equal($('user-menu').hidden, true, 'Klick daneben schließt das Menü');
  click($('btn-user'));
  click($('btn-user'));
  assert.equal($('user-menu').hidden, true, 'zweiter Klick schließt');
  ok('Menü öffnet und schließt (Schalter, Escape, Klick daneben)');

  // Passwortseite über das Menü
  click($('btn-user'));
  window.location.hash = $('user-menu').querySelector('a[href="#/account/password"]').getAttribute('href');
  await until(() => visible('view-password'), 'Passwortseite');
  assert.equal(visible('view-home'), false);
  assert.equal($('user-menu').hidden, true, 'Menü schließt bei Navigation');
  assert.equal(document.title, 'Passwort ändern – Leseliste');
  const pw = $('form-password');
  pw.elements.current.value = 'passwort-12345';
  pw.elements.new.value = 'neues-passwort-1';
  pw.elements.repeat.value = 'anderes-passwort-2';
  pw.requestSubmit();
  await until(() => visible('password-error'), 'Fehler bei ungleicher Wiederholung');
  assert.match($('password-error').textContent, /stimmen nicht überein/);
  pw.elements.repeat.value = 'neues-passwort-1';
  pw.elements.current.value = 'falsch-falsch-1';
  pw.requestSubmit();
  await until(() => /falsch/i.test($('password-error').textContent), 'Fehler bei falschem Passwort');
  ok('Passwortseite zeigt Fehler');

  // Abbrechen führt zurück zur Liste, Formular wird geleert
  const cancel = $('view-password').querySelector('a.button-link');
  assert.equal(cancel.getAttribute('href'), '#/');
  assert.equal(cancel.textContent, 'Abbrechen');
  window.location.hash = cancel.getAttribute('href');
  await until(() => visible('view-home'), 'Liste nach Abbruch');
  assert.equal($('home-notice').hidden, true);
  window.location.hash = '#/account/password';
  await until(() => visible('view-password'), 'Passwortseite erneut');
  assert.equal(pw.elements.current.value, '', 'Formular ist leer');
  assert.equal($('password-error').hidden, true, 'alte Fehlermeldung ist weg');
  ok('Abbrechen bringt zurück zur Liste, Formular ist beim nächsten Öffnen leer');

  // Erfolgreich ändern
  pw.elements.current.value = 'passwort-12345';
  pw.elements.new.value = 'neues-passwort-1';
  pw.elements.repeat.value = 'neues-passwort-1';
  pw.requestSubmit();
  await until(() => visible('view-home'), 'Liste nach Änderung');
  assert.equal($('home-notice').hidden, false);
  assert.equal($('home-notice').textContent, 'Das Passwort wurde geändert.');
  assert.equal(window.location.hash, '#/');
  ok('Passwort ändern führt zur Liste mit Hinweis');
  // Hinweis erscheint nur einmal
  window.location.hash = '#/account/delete';
  await until(() => visible('view-delete'), 'Löschseite');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal($('home-notice').hidden, true, 'Hinweis verschwindet');

  // Neues Passwort gilt
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'Logout');
  $('tab-login').click();
  auth.elements.email.value = 'konto@example.org';
  auth.elements.password.value = 'passwort-12345';
  auth.requestSubmit();
  await until(() => visible('auth-error'), 'altes Passwort abgelehnt');
  auth.elements.password.value = 'neues-passwort-1';
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Login mit neuem Passwort');
  ok('nur das neue Passwort funktioniert');

  // Konto löschen: Seite, Abbruch, falsches Passwort, Erfolg
  window.location.hash = '#/account/delete';
  await until(() => visible('view-delete'), 'Löschseite');
  assert.equal(document.title, 'Konto löschen – Leseliste');
  window.location.hash = $('view-delete').querySelector('a.button-link').getAttribute('href');
  await until(() => visible('view-home'), 'Abbruch Löschen');
  assert.equal(await (await fetch('/api/me')).status, 200, 'Konto besteht noch');
  window.location.hash = '#/account/delete';
  await until(() => visible('view-delete'), 'Löschseite');
  $('form-delete').elements.password.value = 'falsch-falsch-1';
  $('form-delete').requestSubmit();
  await until(() => visible('delete-error'), 'Fehler beim Löschen');
  assert.equal(visible('view-delete'), true);
  $('form-delete').elements.password.value = 'neues-passwort-1';
  $('form-delete').requestSubmit();
  await until(() => visible('view-auth'), 'Anmeldung nach Löschen');
  assert.equal(await (await fetch('/api/me')).status, 401);
  assert.equal($('form-delete').elements.password.value, '');
  ok('Konto löschen: Abbruch, falsches Passwort, Erfolg');

  // Direktaufruf der Kontoseite ohne Anmeldung zeigt die Anmeldung
  window.location.hash = '#/account/password';
  await sleep(100);
  assert.equal(visible('view-auth'), true);
  assert.equal(visible('view-password'), false);
  ok('Kontoseiten sind ohne Anmeldung nicht sichtbar');
});
