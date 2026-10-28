// Der JUnit-Reporter von Node schreibt Tests ohne `describe` direkt unter <testsuites>.
// dorny/test-reporter erwartet aber <testsuite>-Elemente. Dieses Skript packt alle
// <testcase>-Elemente in ein <testsuite> mit Zählern. Aufruf: node junit-wrap.mjs junit.xml
import { readFileSync, writeFileSync } from 'node:fs';

const file = process.argv[2] ?? 'junit.xml';
const xml = readFileSync(file, 'utf8');
if (/<testsuite[\s>]/.test(xml)) process.exit(0); // schon im erwarteten Format

const body = xml.replace(/^[\s\S]*?<testsuites>/, '').replace(/<\/testsuites>\s*$/, '');
const cases = body.match(/<testcase\b/g)?.length ?? 0;
const failures = body.match(/<failure\b/g)?.length ?? 0;
const skipped = body.match(/<skipped\b/g)?.length ?? 0;
const time = [...body.matchAll(/<testcase\b[^>]*\stime="([\d.]+)"/g)].reduce((s, m) => s + Number(m[1]), 0);

const out = `<?xml version="1.0" encoding="utf-8"?>
<testsuites>
<testsuite name="Frontend" tests="${cases}" failures="${failures}" errors="0" skipped="${skipped}" time="${time.toFixed(3)}">${body}</testsuite>
</testsuites>
`;
writeFileSync(file, out);
