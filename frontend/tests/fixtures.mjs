// Testseiten, die der Testserver ausliefert (nur Text, kein Netzwerk nötig).

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
