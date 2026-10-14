// Link „Zur Liste“ im Lesemodus: oben normal, beim Weiterscrollen als schwebende Bubble,
// am Textende unten ein normaler Link und keine Bubble mehr.
//
// jsdom hat kein Layout, darum ein einfaches simuliertes: Der Link oben liegt bei 100 px (30 px
// hoch), der Artikeltext beginnt bei 300 px, zwanzig Absätze à 200 px, der Link unten liegt
// 100 px unter dem Text, das Fenster ist 800 px hoch. Die Bubble ist also ab 130 px Scrollweg
// zu sehen und verschwindet, sobald der Link unten ins Bild kommt (ab 3.600 px).
import { after, afterEach, test } from 'node:test';
import assert from 'node:assert/strict';

import { LANG } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, setOffline, site, teardown } = await setup({ '/lang.html': LANG });
after(teardown);
afterEach(() => setOffline(false));
const spec = (name, fn) => test(name, { timeout: 60_000 }, fn);

const { floatVisible } = await import('../public/js/backlink.js');

const DOC_TOP = 300;
const BLOCK_H = 200;
const VIEWPORT_H = 800;
const TOP_LINK = 100;
let scrollY = 0;
let extra = 0;

const root = () => $('reader-content');
const paragraphs = () => [...root().querySelectorAll('p')];
const bottomLinkTop = () => DOC_TOP + extra + paragraphs().length * BLOCK_H + 100;
const maxScroll = () => bottomLinkTop() + 300 - VIEWPORT_H;

function box(top, height) {
  return { top, bottom: top + height, left: 0, right: 600, width: 600, height, x: 0, y: top };
}

Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
window.innerHeight = VIEWPORT_H;
Object.defineProperty(document.documentElement, 'clientHeight', { configurable: true, get: () => VIEWPORT_H });
window.scrollTo = (first, second) => {
  const top = typeof first === 'object' ? first.top : second;
  scrollY = Math.max(0, Math.min(maxScroll(), top));
};
window.Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
  const view = $('view-reader');
  if (view.hidden || !view.contains(this)) return box(0, 0);
  if (this === $('reader-back')) return box(TOP_LINK - scrollY, 30);
  if (this === $('reader-back-bottom')) return box(bottomLinkTop() - scrollY, 30);
  if (this === root()) return box(DOC_TOP + extra - scrollY, paragraphs().length * BLOCK_H);
  const index = paragraphs().indexOf(this);
  if (index >= 0) return box(DOC_TOP + extra + index * BLOCK_H - scrollY, BLOCK_H);
  return box(0, 0);
};

function userScroll(y) {
  window.dispatchEvent(new window.Event('wheel'));
  scrollY = y;
  window.dispatchEvent(new window.Event('scroll'));
}
const frame = () => sleep(60); // ein Bild abwarten (requestAnimationFrame in jsdom)
const bubble = () => $('reader-back-float');

async function openReader(id) {
  window.location.hash = `#/article/${id}`;
  await until(() => visible('view-reader') && root().textContent.length > 200, 'Lesemodus');
  await sleep(60);
}

spec('Bubble erscheint beim Scrollen und verschwindet am Seitenende', async (t) => {
  const ok = steps(t);

  // reine Entscheidung
  assert.equal(floatVisible({ topBottom: 30, bottomTop: 4000, viewport: 800 }), false);
  assert.equal(floatVisible({ topBottom: 0, bottomTop: 4000, viewport: 800 }), true);
  assert.equal(floatVisible({ topBottom: -500, bottomTop: 799, viewport: 800 }), false);
  assert.equal(floatVisible({ topBottom: -500, bottomTop: 800, viewport: 800 }), true);
  ok('Entscheidungsregel');

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'zurueck@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  $('add-url').value = site.url('/lang.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const id = Number(
    document.querySelector('#article-list .article-main').getAttribute('href').split('/').pop(),
  );

  // alle drei Links führen zur Liste
  for (const name of ['reader-back', 'reader-back-float', 'reader-back-bottom']) {
    assert.equal($(name).getAttribute('href'), '#/', name);
  }
  assert.equal(bubble().hidden, true, 'in der Liste keine Bubble');
  ok('alle drei Links zeigen auf die Liste');

  // oben: normaler Link, keine Bubble
  await openReader(id);
  await frame();
  assert.equal(scrollY, 0);
  assert.equal(bubble().hidden, true, 'ganz oben keine Bubble');
  userScroll(100); // Link oben noch zu sehen (Unterkante 30 px)
  await frame();
  assert.equal(bubble().hidden, true, 'Link oben noch im Bild');
  ok('oben steht nur der normale Link');

  // weiter unten: Bubble
  userScroll(1000);
  await frame();
  assert.equal(bubble().hidden, false, 'Bubble beim Weiterscrollen');
  userScroll(2000);
  await frame();
  assert.equal(bubble().hidden, false);
  ok('beim Weiterscrollen erscheint die Bubble');

  // Seitenende: Link unten im Bild, Bubble weg
  userScroll(3500);
  await frame();
  assert.equal(bubble().hidden, false, 'Link unten noch nicht im Bild');
  userScroll(3650);
  await frame();
  assert.equal(bubble().hidden, true, 'Link unten im Bild: keine Bubble');
  userScroll(maxScroll());
  await frame();
  assert.equal(bubble().hidden, true, 'ganz unten keine Bubble');
  ok('am Seitenende ist die Bubble weg, der Link unten ist zu sehen');

  // wieder hoch
  userScroll(2000);
  await frame();
  assert.equal(bubble().hidden, false, 'wieder hochgescrollt: Bubble da');
  userScroll(0);
  await frame();
  assert.equal(bubble().hidden, true, 'ganz oben wieder keine Bubble');
  ok('beim Zurückscrollen kommt die Bubble wieder und verschwindet oben');

  // viele Scroll-Ereignisse in einem Bild: ein Ergebnis
  for (let y = 1000; y < 1100; y += 10) userScroll(y);
  await frame();
  assert.equal(bubble().hidden, false);
  ok('mehrere Ereignisse pro Bild ergeben ein richtiges Ergebnis');

  // nachladende Bilder verschieben das Seitenende
  userScroll(3650);
  await frame();
  assert.equal(bubble().hidden, true);
  extra = 1000;
  root().dispatchEvent(new window.Event('load')); // load blubbert nicht, Capture-Listener genügt
  await frame();
  assert.equal(bubble().hidden, false, 'Seitenende ist weiter weg gerückt');
  extra = 0;
  ok('nachladende Inhalte werden berücksichtigt');

  // Ansicht verlassen: sofort weg; Wiederöffnen blitzt nicht an der alten Stelle
  userScroll(2000);
  await frame();
  assert.equal(bubble().hidden, false);
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal(bubble().hidden, true, 'in der Liste keine Bubble');
  await frame();
  assert.equal(bubble().hidden, true);
  userScroll(5000);
  await frame();
  assert.equal(bubble().hidden, true, 'Scrollen in der Liste zeigt die Bubble nicht');
  scrollY = 0;
  ok('in der Liste gibt es keine Bubble');
});
