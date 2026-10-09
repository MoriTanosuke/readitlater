// Gemeinsame Hilfen für die Frontend-Tests: echtes Backend und Testserver starten
// und das echte Frontend (index.html + js/app.js) in jsdom laden.
//
// Voraussetzung: Das Backend ist gebaut (`cargo build` in backend/). Ein anderes Programm
// kann über die Umgebungsvariable BACKEND_BIN angegeben werden.

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { JSDOM } from 'jsdom';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const ROOT = path.resolve(HERE, '../..');
export const PUBLIC = path.join(ROOT, 'frontend/public');
const BACKEND_BIN =
  process.env.BACKEND_BIN ?? path.join(ROOT, 'backend/target/debug/readlater-backend');

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createNetServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Kleiner Webserver, der feste Seiten ausliefert: { '/artikel.html': '<html>…' }. */
export async function startSite(pages) {
  const server = createServer((request, response) => {
    const page = pages[request.url];
    if (page === undefined) {
      response.writeHead(404).end('nicht gefunden');
      return;
    }
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' }).end(page);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return {
    url: (route) => `http://127.0.0.1:${port}${route}`,
    stop: () => new Promise((resolve) => server.close(resolve)),
  };
}

/** Startet das echte Backend mit frischer Datenbank. Der SSRF-Schutz ist aus (Testserver ist lokal). */
export async function startBackend(env = {}) {
  if (!existsSync(BACKEND_BIN)) {
    throw new Error(
      `Backend nicht gefunden: ${BACKEND_BIN}\nBitte zuerst "cargo build" in backend/ ausführen.`,
    );
  }
  const dir = mkdtempSync(path.join(tmpdir(), 'leseliste-test-'));
  const port = await freePort();
  const child = spawn(BACKEND_BIN, [], {
    env: {
      ...process.env,
      DATABASE_PATH: path.join(dir, 'app.db'),
      BIND_ADDR: `127.0.0.1:${port}`,
      FETCH_ALLOW_PRIVATE: 'true',
      REGISTRATION_ENABLED: 'true',
      COOKIE_SECURE: 'false',
      RUST_LOG: 'warn',
      ...env,
    },
    stdio: ['ignore', 'inherit', 'inherit'],
  });
  let exited = false;
  child.once('exit', () => {
    exited = true;
  });

  const url = `http://127.0.0.1:${port}`;
  const stop = async () => {
    if (!exited) {
      child.kill('SIGTERM');
      await new Promise((resolve) => child.once('exit', resolve));
    }
    rmSync(dir, { recursive: true, force: true });
  };

  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (exited) throw new Error('Das Backend wurde unerwartet beendet.');
    try {
      if ((await fetch(`${url}/api/health`)).ok) return { url, stop };
    } catch {
      // noch nicht bereit
    }
    await sleep(100);
  }
  await stop();
  throw new Error('Das Backend ist nicht rechtzeitig gestartet.');
}

/**
 * Lädt das Frontend in jsdom. Anfragen an /api/... gehen mit Cookie-Speicher an das echte
 * Backend. Setzt die globalen Variablen, die app.js erwartet (document, location, …).
 */
export async function openApp(backendUrl) {
  const html = readFileSync(path.join(PUBLIC, 'index.html'), 'utf8').replace(
    /<script[^>]*><\/script>/,
    '',
  );
  const dom = new JSDOM(html, { url: 'http://app.test/', pretendToBeVisual: true });
  const { window } = dom;
  Object.assign(globalThis, {
    window,
    document: window.document,
    Node: window.Node,
    DOMParser: window.DOMParser,
    location: window.location,
    history: window.history,
  });
  window.scrollTo = () => {};
  window.confirm = () => true;

  const realFetch = globalThis.fetch;
  let cookie = '';
  globalThis.fetch = async (route, init = {}) => {
    const headers = { ...init.headers, ...(cookie ? { cookie } : {}) };
    const response = await realFetch(backendUrl + route, { ...init, headers });
    for (const header of response.headers.getSetCookie()) {
      const pair = header.split(';')[0];
      cookie = pair.endsWith('=') ? '' : pair;
    }
    return response;
  };

  await import(pathToFileURL(path.join(PUBLIC, 'js/app.js')).href);

  const $ = (id) => document.getElementById(id);
  return {
    $,
    visible: (id) => !$(id).hidden,
    /** Wartet, bis die Bedingung erfüllt ist; sonst Fehler mit dem Seitentext. */
    async until(condition, label, timeout = 8000) {
      const start = Date.now();
      while (Date.now() - start < timeout) {
        if (condition()) return;
        await sleep(25);
      }
      throw new Error(
        `Zeitüberschreitung: ${label}\nSeite: ${document.body.textContent.replace(/\s+/g, ' ').slice(0, 400)}`,
      );
    },
    close: () => window.close(),
  };
}

/** Startet Backend und Frontend; `pages` sind die Seiten des Testservers. */
export async function setup(pages = {}) {
  const site = await startSite(pages);
  let backend;
  try {
    backend = await startBackend();
    const app = await openApp(backend.url);
    return {
      ...app,
      site,
      async teardown() {
        app.close();
        await backend.stop();
        await site.stop();
      },
    };
  } catch (error) {
    await backend?.stop();
    await site.stop();
    throw error;
  }
}

/** Gibt Schritt-Meldungen im Testbericht aus. */
export const steps = (t) => (text) => t.diagnostic(text);
