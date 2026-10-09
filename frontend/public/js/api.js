// Dünner Wrapper um fetch für die Backend-API (gleiche Origin über Caddy).

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

async function request(method, path, body) {
  const headers = { 'X-Requested-With': 'fetch' }; // CSRF-Schutz im Backend
  if (body !== undefined) headers['Content-Type'] = 'application/json';

  let response;
  try {
    response = await fetch(`/api${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'same-origin',
    });
  } catch {
    throw new ApiError(0, 'Server nicht erreichbar. Bitte später erneut versuchen.');
  }

  if (response.status === 204) return null;

  let data = null;
  try {
    data = await response.json();
  } catch {
    // Antwort ohne JSON-Body
  }
  if (!response.ok) {
    throw new ApiError(response.status, data?.error ?? `Fehler ${response.status}`);
  }
  return data;
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
  /** filter: { q, read ('true' | 'false' | ''), tag } */
  listArticles: (limit, offset, filter = {}) => {
    const params = new URLSearchParams({ limit, offset });
    if (filter.q) params.set('q', filter.q);
    if (filter.read) params.set('read', filter.read);
    if (filter.tag) params.set('tag', filter.tag);
    return request('GET', `/articles?${params}`);
  },
  listTags: () => request('GET', '/tags'),
  /** changes: { is_read?, tags? } */
  updateArticle: (id, changes) => request('PATCH', `/articles/${encodeURIComponent(id)}`, changes),
  addArticle: (url) => request('POST', '/articles', { url }),
  getArticle: (id) => request('GET', `/articles/${encodeURIComponent(id)}`),
  deleteArticle: (id) => request('DELETE', `/articles/${encodeURIComponent(id)}`),
};
