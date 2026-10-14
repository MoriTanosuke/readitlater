// Testseiten, die der Testserver ausliefert (nur Text, kein Netzwerk nötig).

import zlib from 'node:zlib';

const PARAGRAPH =
  'Dies ist ein ausführlicher Absatz mit genug Text, damit die Extraktion ihn sicher als ' +
  'Hauptinhalt der Seite erkennt und nicht als Beiwerk verwirft.';

/** Artikel mit absichtlich gefährlichen Teilen (Skript, Event-Handler, javascript:-Link). */
export const ARTIKEL = `<!doctype html><html><head><title>E2E Testartikel</title></head><body>
<nav><a href="/">Start</a></nav>
<article>
<p>${PARAGRAPH}</p>
<p>${PARAGRAPH}</p>
<p>${PARAGRAPH}</p>
<p>${PARAGRAPH}</p>
<script>window.pwned = 1</script>
<p onclick="steal()">Link: <a href="javascript:alert(1)">böse</a> und <a href="https://example.org/ziel">gut</a>.</p>
<img src="https://example.org/bild.png" onerror="steal()">
</article><footer>Impressum</footer></body></html>`;

/** Langer Artikel mit zwanzig gleich langen Absätzen (für die Leseposition). */
export const LANG = `<!doctype html><html><head><title>Langer Artikel</title></head><body>
<article>
${Array.from({ length: 20 }, (_, i) => `<p>Absatz ${String(i + 1).padStart(2, '0')}: ${PARAGRAPH}</p>`).join('\n')}
</article></body></html>`;

/** Zweiter Artikel mit eigenem Suchwort („Kaffeebohnen“). */
export const KAFFEE = `<!doctype html><html><head><title>Kaffee und Bohnen</title></head><body>
<nav><a href="/">Start</a></nav>
<article>
${Array.from({ length: 4 }, () => `<p>Kaffeebohnen werden geröstet, ein ausführlicher Absatz mit genug Text, damit die Extraktion ihn sicher als Hauptinhalt der Seite erkennt und nicht als Beiwerk verwirft.</p>`).join('\n')}
</article><footer>Impressum</footer></body></html>`;

/** Englischer Artikel (die Seite behauptet absichtlich lang="de"). */
export const ENGLISCH = `<!doctype html><html lang="de"><head><title>English Article</title></head><body>
<article>
${Array.from({ length: 4 }, () => '<p>The government announced on Wednesday that it would speed up the expansion of renewable energy, but critics say that the funding has not been settled and many questions remain unanswered.</p>').join('\n')}
</article></body></html>`;

/** Artikel mit zwei Bildern: eines gibt es auf dem Testserver, das andere nicht (404). */
export const MIT_BILDERN = `<!doctype html><html><head><title>Artikel mit Bildern</title></head><body>
<article>
<p>${PARAGRAPH}</p>
<p><img src="/bild/da.png" alt="vorhanden"></p>
<p>${PARAGRAPH}</p>
<p><img src="/bild/fehlt.png" alt="fehlt"></p>
<p>${PARAGRAPH}</p>
<p>${PARAGRAPH}</p>
</article></body></html>`;

/** Gültiges PNG (einfarbig) ohne Hilfsbibliothek, Node 22 bringt zlib.crc32 mit. */
export function makePng(width, height) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(zlib.crc32(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // 8 Bit
  header[9] = 2; // RGB
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3, 0x66)]);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
