// Frontend-Ablauf: Artikel von Hand hinzufügen (Adresse plus eingefügter Text, ohne Abruf).
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { setup, steps } from './helpers.mjs';

// Der Testserver kennt die Adresse nicht: Es darf nichts abgerufen werden.
const { $, visible, until, teardown } = await setup({});
after(teardown);

const TEXT =
  'Erster Absatz mit <b>spitzen</b> Klammern & einem Und-Zeichen.\n' +
  'Zweiter Absatz, der lang genug ist, damit der Text als Artikel gilt und gespeichert wird.';

test('Artikel von Hand hinzufügen', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const auth = $('form-auth');
  auth.elements.email.value = 'hand@example.org';
  auth.elements.password.value = 'passwort-12345';
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  // Eine schon getippte Adresse wird in die Handeingabe übernommen.
  $('add-url').value = 'https://bezahlt.example/artikel';
  $('link-manual').click();
  await until(() => visible('view-manual'), 'Seite „von Hand“');
  const form = $('form-manual');
  assert.equal(form.elements.url.value, 'https://bezahlt.example/artikel');
  ok('Link öffnet die Seite und übernimmt die Adresse');

  // Speichern
  form.elements.title.value = 'Mein Handartikel';
  form.elements.content.value = TEXT;
  form.requestSubmit();
  await until(() => visible('view-home'), 'Liste nach dem Speichern');
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel in der Liste');
  assert.equal(document.querySelector('#article-list .article-title').textContent, 'Mein Handartikel');
  assert.equal($('home-notice').textContent, 'Artikel gespeichert.');
  assert.equal($('add-url').value, '', 'Adressfeld der Liste wird geleert');
  ok('Artikel erscheint in der Liste');

  // Lesemodus: Text steht als Text da, nicht als HTML
  window.location.hash = document.querySelector('#article-list .article-main').getAttribute('href');
  await until(() => visible('view-reader') && $('reader-content').children.length > 0, 'Lesemodus');
  const content = $('reader-content');
  assert.equal(content.querySelectorAll('p').length, 2);
  assert.equal(content.querySelector('b'), null);
  assert.match(content.textContent, /<b>spitzen<\/b> Klammern & einem/);
  ok('Lesemodus zeigt den Text maskiert, ein Absatz je Zeile');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'zurück zur Liste');

  // Dieselbe Adresse noch einmal: Fehlermeldung, Eingaben bleiben
  window.location.hash = '#/add';
  await until(() => visible('view-manual'), 'Seite „von Hand“ erneut');
  form.elements.url.value = 'https://bezahlt.example/artikel#anker';
  form.elements.content.value = TEXT;
  form.requestSubmit();
  await until(() => visible('manual-error'), 'Fehlermeldung bei Duplikat');
  assert.match($('manual-error').textContent, /bereits gespeichert/);
  assert.equal(form.elements.content.value, TEXT);
  ok('doppelte Adresse zeigt Fehlermeldung und behält die Eingaben');

  // Zu kurzer Text
  form.elements.url.value = 'https://bezahlt.example/kurz';
  form.elements.content.value = 'zu kurz';
  form.requestSubmit();
  await until(() => /kein lesbarer Artikeltext/.test($('manual-error').textContent), 'Fehlermeldung bei kurzem Text');
  ok('zu kurzer Text wird abgelehnt');

  // Abbrechen führt zurück, ohne zu speichern
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Abbrechen');
  assert.equal(document.querySelectorAll('#article-list li').length, 1);
  ok('Abbrechen speichert nichts');
});
