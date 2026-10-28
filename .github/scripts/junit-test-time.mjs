// dorny/test-reporter zeigt die Zeit einzelner Testfälle nicht an, obwohl sie im JUnit-Bericht
// steht (Attribut `time`). Dieses Skript hängt sie an den Testnamen an, z. B. „mein_test (12 ms)“,
// damit sie in der Testliste des Berichts erscheint. Aufruf: node junit-test-time.mjs junit.xml [...]
import { readFileSync, writeFileSync } from 'node:fs';

function format(seconds) {
  if (seconds < 1) return `${Math.round(seconds * 1000)} ms`;
  return `${seconds.toFixed(2)} s`;
}

for (const file of process.argv.slice(2)) {
  const xml = readFileSync(file, 'utf8');
  const out = xml.replace(/<testcase\b[^>]*?\/?>/g, (tag) => {
    const time = /\stime="([\d.]+)"/.exec(tag);
    if (!time) return tag;
    return tag.replace(/(\sname=")([^"]*)(")/, (m, a, name, b) =>
      / \(\d+(\.\d+)? (ms|s)\)$/.test(name) ? m : `${a}${name} (${format(Number(time[1]))})${b}`,
    );
  });
  writeFileSync(file, out);
}
