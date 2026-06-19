// api.js — All fetch calls in one place
const BASE = '/api/v1';

class ApiError extends Error {
  constructor(message, code, status) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

async function request(method, path, body) {
  const opts = {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  };
  const res = await fetch(BASE + path, opts);
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText, code: 'HTTP_ERROR' }));
    throw new ApiError(err.error || res.statusText, err.code, res.status);
  }
  return res.status === 204 ? null : res.json();
}

export const api = {
  stats:          ()        => request('GET',    '/stats'),
  models:         (params)  => request('GET',    '/models?' + new URLSearchParams(params || {})),
  model:          (hash)    => request('GET',    `/models/${hash}`),
  dupes:          ()        => request('GET',    '/dupes'),
  dedup:          (body)    => request('POST',   '/dupes/dedup', body),
  downloads:      ()        => request('GET',    '/downloads'),
  scan:           (body)    => request('POST',   '/scan/trigger', body || {}),
  gcPreview:      ()        => request('POST',   '/gc/preview'),
  gcRun:          (body)    => request('POST',   '/gc/run', body || {}),
  settings:       ()        => request('GET',    '/settings'),
  saveSettings:   (body)    => request('PUT',    '/settings', body),
};

export { ApiError };
