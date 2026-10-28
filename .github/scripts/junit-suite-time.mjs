// cargo-nextest schreibt kein `time` an die <testsuite>-Elemente. dorny/test-reporter zeigt
// die Ausführungszeit der Suites deshalb als „NaNms“. Dieses Skript ergänzt `time` als Summe der
// Zeiten der enthaltenen <testcase>-Elemente (Sekunden). Aufruf: node junit-suite-time.mjs junit.xml
import { readFileSync, writeFileSync } from 'node:fs';

const file = process.argv[2];
if (!file) {
  console.error('Aufruf: node junit-suite-time.mjs <junit.xml>');
  process.exit(2);
}

const xml = readFileSync(file, 'utf8');
const out = xml.replace(/<testsuite\b([^>]*)>([\s\S]*?)<\/testsuite>/g, (match, attrs, body) => {
  if (/\stime="/.test(attrs)) return match; // schon vorhanden
  const time = [...body.matchAll(/<testcase\b[^>]*?\stime="([\d.]+)"/g)].reduce((sum, m) => sum + Number(m[1]), 0);
  return `<testsuite${attrs} time="${time.toFixed(3)}">${body}</testsuite>`;
});
writeFileSync(file, out);
