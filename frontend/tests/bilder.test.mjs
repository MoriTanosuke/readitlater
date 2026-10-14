// Lokale Bilder: Das echte Backend holt die Bilder im Hintergrund, verkleinert sie und liefert
// sie aus; das Frontend tauscht beim Anzeigen die Adresse. Bilder, die sich nicht laden lassen,
// bleiben Original-Links, „Bilder nachladen“ versucht es erneut.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { MIT_BILDERN, makePng } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const pages = {
  '/artikel.html': MIT_BILDERN,
  '/bild/da.png': { type: 'image/png', body: makePng(2000, 1200) },
  // '/bild/fehlt.png' gibt es zunächst nicht (404).
};
const { $, visible, until, site, teardown } = await setup(pages);
after(teardown);

const images = () => [...$('reader-content').querySelectorAll('img')];
const byAlt = (alt) => images().find((img) => img.getAttribute('alt') === alt);

async function serverImages(id) {
  const response = await fetch(`/api/articles/${id}`);
  return (await response.json()).images;
}

async function untilAsync(check, label, timeout = 15_000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await check()) return;
    await sleep(100);
  }
  throw new Error(`Zeitüberschreitung: ${label}`);
}

test('Bilder werden lokal abgelegt, verkleinert und im Lesemodus verwendet', { timeout: 90_000 }, async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'bilder@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  $('add-url').value = site.url('/artikel.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const id = Number(
    document.querySelector('#article-list .article-main').getAttribute('href').split('/').pop(),
  );

  // ---- Direkt nach dem Speichern: Original-Adressen, die Anzeige wartet nicht
  window.location.hash = `#/article/${id}`;
  await until(() => visible('view-reader') && images().length === 2, 'Lesemodus mit zwei Bildern');
  await sleep(100);
  assert.equal(byAlt('vorhanden').getAttribute('src'), site.url('/bild/da.png'));
  ok('beim ersten Öffnen stehen die Original-Adressen im Text');

  // ---- Der Server verarbeitet im Hintergrund (nur das erste Bild gibt es)
  await untilAsync(async () => Object.keys(await serverImages(id)).length === 1, 'Bild fertig');
  const entry = (await serverImages(id))[site.url('/bild/da.png')];
  assert.match(entry.src, new RegExp(`^/api/articles/${id}/images/\\d+$`));
  assert.deepEqual([entry.width, entry.height], [1600, 960], 'auf 1600 px verkleinert');
  const image = await fetch(entry.src);
  assert.equal(image.status, 200);
  assert.equal(image.headers.get('content-type'), 'image/png');
  ok('das Bild liegt verkleinert auf dem Server (1600 × 960)');

  // ---- Erneutes Öffnen: Text unverändert, Bild kommt von hier, das fehlende bleibt Original
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  window.location.hash = `#/article/${id}`;
  await until(() => visible('view-reader') && images().length === 2, 'Lesemodus erneut');
  await until(() => byAlt('vorhanden').getAttribute('src') === entry.src, 'Bild getauscht');
  assert.equal(byAlt('vorhanden').getAttribute('width'), '1600');
  assert.equal(byAlt('vorhanden').getAttribute('height'), '960');
  assert.equal(byAlt('fehlt').getAttribute('src'), site.url('/bild/fehlt.png'));
  assert.equal($('reader-refresh-images').hidden, false, 'es gibt noch ein Original-Bild');
  ok('lokales Bild mit Maßen, fehlendes bleibt Original, „Bilder nachladen“ ist sichtbar');

  // ---- Nachladen: Das Bild ist inzwischen auf dem Testserver erreichbar
  pages['/bild/fehlt.png'] = { type: 'image/png', body: makePng(300, 200) };
  $('reader-refresh-images').click();
  await until(() => !$('reader-notice').hidden, 'Hinweis beim Nachladen');
  await until(
    () => byAlt('fehlt').getAttribute('src').startsWith(`/api/articles/${id}/images/`),
    'nachgeladenes Bild getauscht',
    20_000,
  );
  await until(() => $('reader-refresh-images').hidden, 'Knopf verschwindet');
  assert.equal(Object.keys(await serverImages(id)).length, 2);
  ok('„Bilder nachladen“ holt das fehlende Bild, danach verschwindet der Knopf');
  window.location.hash = '#/';
});
