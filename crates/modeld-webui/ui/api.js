// api.js — All fetch calls in one place
const BASE = '/api/v1';

class ApiError extends Error {
  constructor(message, code, status) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

const TOKEN_KEY = 'modeld_token';

// Every request carries the CSRF header; the server rejects browser-originated
// writes that lack it. The bearer token (when the server needs one) lives in
// localStorage and is filled in by the prompt below on the first 401.
function buildHeaders(hasBody) {
  const headers = { 'X-Modeld-CSRF': '1' };
  if (hasBody) headers['Content-Type'] = 'application/json';
  const token = localStorage.getItem(TOKEN_KEY);
  if (token) headers['Authorization'] = 'Bearer ' + token;
  return headers;
}

async function send(method, path, body, retried) {
  const res = await fetch(BASE + path, {
    method,
    headers: buildHeaders(Boolean(body)),
    body: body ? JSON.stringify(body) : undefined,
  });

  if (res.status === 401 && !retried) {
    const entered = window.prompt('This modeld server requires an access token:');
    if (entered && entered.trim()) {
      localStorage.setItem(TOKEN_KEY, entered.trim());
      return send(method, path, body, true);
    }
  }

  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText, code: 'HTTP_ERROR' }));
    throw new ApiError(err.error || res.statusText, err.code, res.status);
  }
  return res.status === 204 ? null : res.json();
}

function request(method, path, body) {
  return send(method, path, body, false);
}

export const api = {
  stats:          ()        => request('GET',    '/stats'),
  models:         (params)  => request('GET',    '/models?' + new URLSearchParams(params || {})),
  model:          (hash)    => request('GET',    `/models/${hash}`),
  dupes:          ()        => request('GET',    '/dupes'),
  dedupPreview:   ()        => request('POST',   '/dedup/preview'),
  dedupApply:     (body)    => request('POST',   '/dedup/apply', body || {}),
  downloads:      ()        => request('GET',    '/downloads'),
  scan:           (body)    => request('POST',   '/scan', body || {}),
  gcPreview:      ()        => request('GET',    '/gc/preview'),
  gcRun:          (body)    => request('POST',   '/gc/run', body || {}),
  settings:       ()        => request('GET',    '/settings'),
  saveSettings:   (body)    => request('PUT',    '/settings', body),
};

export { ApiError };
