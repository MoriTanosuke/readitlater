// Liste merkt ihren Scrollstand: Wer aus dem Lesemodus (oder von einer Kontoseite) zurückkehrt,
// steht wieder dort, wo er die Liste verlassen hat, nicht auf dem Scrollstand des Artikels.
//
// jsdom hat kein Layout; `window.scrollY` und `window.scrollTo` werden darum durch einen einfachen
// Wert ersetzt, der für alle Ansichten gemeinsam gilt, wie das Fenster im echten Browser.
import { after, afterEach, test } from 'node:test';
import assert from 'node:assert/strict';

import { LANG } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, setOffline, site, teardown } = await setup({ '/lang.html': LANG });
after(teardown);
afterEach(() => setOffline(false));
const spec = (name, fn) => test(name, { timeout: 60_000 }, fn);

let scrollY = 0;
Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
window.scrollTo = (first, second) => {
  scrollY = Math.max(0, typeof first === 'object' ? first.top : second);
};

spec('Liste behält ihren Scrollstand nach dem Lesen eines Artikels', async (t) => {
  const ok = steps(t);

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'listenposition@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  $('add-url').value = site.url('/lang.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const id = Number(
    document.querySelector('#article-list .article-main').getAttribute('href').split('/').pop(),
  );

  // Liste ist gescrollt, Artikel öffnen: der Artikel startet oben
  scrollY = 640;
  window.location.hash = `#/article/${id}`;
  await until(() => visible('view-reader') && $('reader-content').textContent.length > 200, 'Lesemodus');
  assert.equal(scrollY, 0, 'Artikel beginnt oben');
  ok('Artikel startet oben');

  // im Artikel weit nach unten, dann zurück zur Liste
  scrollY = 5000;
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal(scrollY, 640, 'Liste steht wieder auf dem alten Stand');
  ok('Liste steht nach der Rückkehr wieder dort wie vorher');

  // der Stand gilt nur einmal: neuer Besuch ohne Umweg über eine andere Ansicht ändert nichts
  scrollY = 100;
  await sleep(50);
  assert.equal(scrollY, 100);
  ok('Wiederherstellung nur einmalig');

  // auch Kontoseiten: Stand merken und wiederherstellen
  scrollY = 320;
  window.location.hash = '#/account/display';
  await until(() => visible('view-display'), 'Darstellungsseite');
  assert.equal(scrollY, 0);
  scrollY = 90;
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal(scrollY, 320);
  ok('Kontoseiten merken den Stand der Liste ebenso');
});
