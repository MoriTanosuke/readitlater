// Tests für frontend/public/js/safe-html.js (bereinigte Darstellung von Artikel-HTML).
import assert from 'node:assert/strict';
import path from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';

import { JSDOM } from 'jsdom';

import { PUBLIC } from './helpers.mjs';

const dom = new JSDOM('<!doctype html><body><div id="c"></div></body>', { url: 'https://app.example/' });
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.Node = dom.window.Node;
globalThis.DOMParser = dom.window.DOMParser;

const { renderSafeHtml, safeUrl } = await import(
  pathToFileURL(path.join(PUBLIC, 'js/safe-html.js')).href
);

const ALLOWED_ATTRS = new Set([
  'title', 'href', 'rel', 'target', 'src', 'alt', 'loading', 'decoding', 'referrerpolicy', 'colspan', 'rowspan',
]);

function render(html, base = 'https://news.example/artikel/1') {
  const container = document.getElementById('c');
  renderSafeHtml(container, html, base);
  for (const el of container.querySelectorAll('*')) {
    for (const attr of el.attributes) {
      assert.ok(ALLOWED_ATTRS.has(attr.name), `unerlaubtes Attribut ${attr.name} an <${el.tagName}>`);
    }
  }
  return container;
}

test('Skripte, Frames, Formulare und SVG verschwinden samt Inhalt', () => {
  const c = render(
    '<p>Text</p><script>window.pwned=1</script><iframe src="https://evil.example"></iframe>' +
      '<form action="https://evil.example"><input name="x"><button>Los</button></form>' +
      '<svg><script>window.pwned=2</script><circle/></svg><style>body{display:none}</style>' +
      '<object data="x"></object><embed src="x"><noscript>nein</noscript>',
  );
  assert.equal(c.innerHTML, '<p>Text</p>');
  assert.equal(globalThis.window.pwned, undefined);
});

test('Event-Handler und Styles werden nicht übernommen', () => {
  const c = render(
    '<p onclick="x()" style="position:fixed" class="a" id="b" onmouseover="y()">Hallo</p>' +
      '<img src="https://img.example/a.png" onerror="z()" style="x:y">',
  );
  assert.equal(c.querySelector('p').attributes.length, 0);
  assert.equal(c.querySelector('img').getAttribute('onerror'), null);
});

test('gefährliche Link-Protokolle werden entfernt (auch getarnt)', () => {
  const c = render(
    '<a href="javascript:alert(1)">a</a><a href="JaVaScRiPt:alert(1)">b</a>' +
      '<a href="java\tscript:alert(1)">c</a><a href=" javascript:alert(1)">d</a>' +
      '<a href="data:text/html,<script>alert(1)</script>">e</a><a href="vbscript:x">f</a>' +
      '<a href="blob:https://x/y">g</a><a href="file:///etc/passwd">h</a>',
  );
  for (const a of c.querySelectorAll('a')) {
    assert.equal(a.getAttribute('href'), null, a.textContent);
  }
  assert.equal(c.querySelectorAll('a').length, 8, 'Text der Links bleibt erhalten');
});

test('erlaubte Links bekommen rel und target, relative werden aufgelöst', () => {
  const c = render(
    '<a href="https://example.org/x?y=1">1</a><a href="/pfad">2</a><a href="mailto:a@b.de">3</a>',
  );
  const [a1, a2, a3] = c.querySelectorAll('a');
  assert.equal(a1.href, 'https://example.org/x?y=1');
  assert.equal(a2.href, 'https://news.example/pfad');
  assert.equal(a3.href, 'mailto:a@b.de');
  for (const a of [a1, a2, a3]) {
    assert.equal(a.rel, 'noopener noreferrer nofollow');
    assert.equal(a.target, '_blank');
  }
});

test('Bilder: nur http(s), mit no-referrer und lazy', () => {
  const c = render(
    '<img src="https://img.example/a.png" alt="Bild"><img src="data:image/svg+xml,<svg onload=alert(1)>">' +
      '<img src="javascript:alert(1)"><img alt="ohne Quelle"><img src="/rel.png">',
  );
  const imgs = [...c.querySelectorAll('img')];
  assert.equal(imgs.length, 2);
  assert.equal(imgs[0].getAttribute('alt'), 'Bild');
  assert.equal(imgs[0].getAttribute('referrerpolicy'), 'no-referrer');
  assert.equal(imgs[0].getAttribute('loading'), 'lazy');
  assert.equal(imgs[1].src, 'https://news.example/rel.png');
});

test('unbekannte Tags werden ausgepackt, Struktur bleibt', () => {
  const c = render('<custom-box><p>Innen <b>fett</b></p></custom-box><section><h2>Titel</h2></section>');
  assert.equal(c.innerHTML, '<p>Innen <b>fett</b></p><h2>Titel</h2>');
});

test('Text wird nie als HTML interpretiert', () => {
  const c = render('<p>&lt;script&gt;alert(1)&lt;/script&gt; &amp; <code>&lt;b&gt;</code></p>');
  assert.equal(c.querySelector('script'), null);
  assert.equal(c.querySelector('p').textContent, '<script>alert(1)</script> & <b>');
});

test('Tabellen: nur gültige colspan/rowspan', () => {
  const c = render(
    '<table><tr><td colspan="2" rowspan="x" onclick="a()">a</td><td colspan="9999">b</td></tr></table>',
  );
  const [a, b] = c.querySelectorAll('td');
  assert.equal(a.getAttribute('colspan'), '2');
  assert.equal(a.getAttribute('rowspan'), null);
  assert.equal(b.getAttribute('colspan'), null);
});

test('mutation XSS-Muster und kaputtes HTML bleiben harmlos', () => {
  const c = render(
    '<noscript><p title="</noscript><img src=x onerror=alert(1)>"></noscript>' +
      '<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>' +
      '<svg></p><style><a title="</style><img src=x onerror=alert(1)>">' +
      '<p><b>unclosed <i>tags',
  );
  assert.equal(c.querySelector('[onerror]'), null);
  assert.equal(c.querySelector('script'), null);
});

test('leere Eingabe und safeUrl', () => {
  assert.equal(render('').innerHTML, '');
  assert.equal(safeUrl('x', 'not a url', ['https:']), null);
  assert.equal(safeUrl('https://a.example/', 'https://b.example/', ['https:']), 'https://a.example/');
});
