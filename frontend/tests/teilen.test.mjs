// Frontend-Ablauf: Token für das Teilen aus anderen Apps anlegen, benutzen und widerrufen.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({ '/artikel.html': ARTIKEL });
after(teardown);

test('Token anlegen, per HTTP teilen, widerrufen', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const auth = $('form-auth');
  auth.elements.email.value = 'teilen@example.org';
  auth.elements.password.value = 'passwort-12345';
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  // Seite öffnen, noch keine Tokens
  window.location.hash = '#/account/share';
  await until(() => visible('view-share'), 'Seite „Teilen aus Apps“');
  assert.equal(document.title, 'Teilen aus Apps – Leseliste');
  await until(() => visible('share-empty'), 'Hinweis ohne Tokens');
  assert.equal($('share-endpoint').textContent, `${window.location.origin}/api/share`);
  assert.equal($('share-created').hidden, true);
  const back = $('view-share').querySelector('a.button-link');
  assert.equal(back.getAttribute('href'), '#/');
  ok('Seite zeigt Adresse der Schnittstelle und leere Tokenliste');

  // Ungültiger Name
  $('form-share').elements.name.value = 'x'.repeat(61);
  $('form-share').requestSubmit();
  // maxlength verhindert das Eintippen, jsdom prüft es aber nicht: das Backend lehnt ab
  await until(() => visible('share-error'), 'Fehler bei zu langem Namen');
  ok('Backend-Fehler werden angezeigt');

  // Token anlegen
  $('form-share').elements.name.value = 'Pixel';
  $('form-share').requestSubmit();
  await until(() => visible('share-created'), 'Anzeige des neuen Tokens');
  const token = $('share-token-value').value;
  assert.match(token, /^lsl_[0-9a-f]{64}$/);
  assert.equal($('share-error').hidden, true);
  await until(() => document.querySelectorAll('#share-list li').length === 1, 'Token in der Liste');
  const item = document.querySelector('#share-list li');
  assert.equal(item.querySelector('strong').textContent, 'Pixel');
  assert.match(item.textContent, /noch nicht benutzt/);
  assert.equal(item.textContent.includes(token), false, 'Liste zeigt den Wert nicht');
  ok('neues Token wird einmalig angezeigt, die Liste enthält nur den Namen');

  // Wie HTTP Shortcuts: nur Authorization-Header, kein X-Requested-With
  const response = await fetch('/api/share', {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'text/plain' },
    body: `Lesenswert ${site.url('/artikel.html')}`,
  });
  assert.equal(response.status, 201);
  assert.equal((await response.json()).title, 'E2E Testartikel');
  ok('Teilen per Token legt den Artikel an');

  // Der Artikel gehört dem Konto; die Seite zeigt die letzte Nutzung
  const saved = await (await fetch('/api/articles')).json();
  assert.equal(saved.length, 1);
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  window.location.hash = '#/account/share';
  await until(() => visible('view-share'), 'Seite erneut');
  assert.equal($('share-created').hidden, true, 'Token wird nicht erneut angezeigt');
  await until(
    () => /zuletzt benutzt am \d\d\.\d\d\.\d{4}/.test(document.querySelector('#share-list li')?.textContent ?? ''),
    'Zeitpunkt der Nutzung',
  );
  ok('Liste zeigt die letzte Nutzung, der Wert bleibt verborgen');

  // Widerrufen
  window.confirm = () => true;
  document.querySelector('#share-list button').click();
  await until(() => visible('share-empty'), 'Liste leer nach Widerruf');
  const denied = await fetch('/api/share', {
    method: 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'text/plain' },
    body: site.url('/artikel.html'),
  });
  assert.equal(denied.status, 401);
  ok('widerrufenes Token wird abgelehnt');
});
