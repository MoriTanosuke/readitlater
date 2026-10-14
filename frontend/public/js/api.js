// Dünner Wrapper um fetch für die Backend-API (gleiche Origin, das Backend liefert auch das Frontend).

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

const TIMEOUT_MS = 20_000; // schlechte Verbindung: lieber früh aufgeben als endlos laden

/**
 * raw: body ist schon fertig (z. B. eine Datei) und wird unverändert als JSON gesendet.
 * timeoutMs: nach dieser Zeit gilt der Server als nicht erreichbar (ApiError mit Status 0).
 * keepalive: die Anfrage läuft auch weiter, wenn die Seite gerade geschlossen wird.
 */
async function request(method, path, body, raw = false, timeoutMs = TIMEOUT_MS, keepalive = false) {
  const headers = { 'X-Requested-With': 'fetch' }; // CSRF-Schutz im Backend
  if (body !== undefined) headers['Content-Type'] = 'application/json';

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const unreachable = () => new ApiError(0, 'Server nicht erreichbar. Bitte später erneut versuchen.');
  try {
    let response;
    try {
      response = await fetch(`/api${path}`, {
        method,
        headers,
        body: body === undefined || raw ? body : JSON.stringify(body),
        credentials: 'same-origin',
        signal: controller.signal,
        keepalive,
      });
    } catch {
      throw unreachable();
    }

    if (response.status === 204) return null;

    let data = null;
    try {
      data = await response.json();
    } catch {
      // Antwort ohne JSON-Body (oder die Verbindung riss beim Lesen ab)
      if (controller.signal.aborted) throw unreachable();
    }
    if (!response.ok) {
      throw new ApiError(response.status, data?.error ?? `Fehler ${response.status}`);
    }
    return data;
  } finally {
    clearTimeout(timer);
  }
}

export const api = {
  me: () => request('GET', '/me'),
  register: (email, password) => request('POST', '/register', { email, password }),
  login: (email, password) => request('POST', '/login', { email, password }),
  logout: () => request('POST', '/logout'),
  changePassword: (currentPassword, newPassword) =>
    request('PUT', '/account/password', {
      current_password: currentPassword,
      new_password: newPassword,
    }),
  deleteAccount: (password) => request('DELETE', '/account', { password }),
  /** file: ausgewählte Exportdatei, der Browser sendet sie direkt. */
  importArticles: (file) => request('POST', '/import', file, true, 300_000),
  listShareTokens: () => request('GET', '/share-tokens'),
  createShareToken: (name) => request('POST', '/share-tokens', { name }),
  deleteShareToken: (id) => request('DELETE', `/share-tokens/${encodeURIComponent(id)}`),
  /** filter: { q, read ('true' | 'false' | ''), tag } */
  listArticles: (limit, offset, filter = {}) => {
    const params = new URLSearchParams({ limit, offset });
    if (filter.q) params.set('q', filter.q);
    if (filter.read) params.set('read', filter.read);
    if (filter.tag) params.set('tag', filter.tag);
    return request('GET', `/articles?${params}`);
  },
  listTags: () => request('GET', '/tags'),
  /** changes: { is_read?, tags?, read_pos? }; options: { keepalive } */
  updateArticle: (id, changes, { keepalive = false } = {}) =>
    request('PATCH', `/articles/${encodeURIComponent(id)}`, changes, false, TIMEOUT_MS, keepalive),
  // Der Server lädt die Seite selbst, das dauert länger als eine normale Anfrage.
  addArticle: (url) => request('POST', '/articles', { url }, false, 60_000),
  getArticle: (id) => request('GET', `/articles/${encodeURIComponent(id)}`),
  deleteArticle: (id) => request('DELETE', `/articles/${encodeURIComponent(id)}`),
  /** Bilder des Artikels (erneut) lokal ablegen lassen; die Antwort nennt die Zahl offener Bilder. */
  refreshImages: (id) =>
    request('POST', `/articles/${encodeURIComponent(id)}/refresh-images`, {}),
  listHighlights: (articleId) =>
    request('GET', `/articles/${encodeURIComponent(articleId)}/highlights`),
  /** highlight: { start, end, quote, prefix, suffix, color, note? } */
  createHighlight: (articleId, highlight) =>
    request('POST', `/articles/${encodeURIComponent(articleId)}/highlights`, highlight),
  /** changes: { color?, note? } (note: null löscht die Notiz) */
  updateHighlight: (id, changes) =>
    request('PATCH', `/highlights/${encodeURIComponent(id)}`, changes),
  deleteHighlight: (id) => request('DELETE', `/highlights/${encodeURIComponent(id)}`),
};
