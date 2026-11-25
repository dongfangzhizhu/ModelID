// session.js — single source of truth for the bearer token and auth prompt (C5).
//
// Before C5 the token key was duplicated in api.js and ws.js, and the 401
// prompt lived inside the fetch layer. Centralising here means:
//   - one storage key, one read path, one clear path;
//   - the prompt can be triggered from either HTTP or WebSocket auth failures;
//   - tests can inject a storage implementation without a DOM.

const TOKEN_KEY = 'modeld_token';

let storage = null;
function store() {
  if (storage) return storage;
  if (typeof localStorage !== 'undefined') return localStorage;
  // Test / non-browser environments: in-memory fallback, never persisted.
  const mem = new Map();
  storage = {
    getItem: (k) => (mem.has(k) ? mem.get(k) : null),
    setItem: (k, v) => mem.set(k, String(v)),
    removeItem: (k) => mem.delete(k),
  };
  return storage;
}

/** Replace the backing store (used by tests). */
export function setSessionStorage(custom) {
  storage = custom;
}

export function getToken() {
  const value = store().getItem(TOKEN_KEY);
  return value && value.trim() ? value.trim() : null;
}

export function setToken(token) {
  const trimmed = String(token || '').trim();
  if (!trimmed) return false;
  store().setItem(TOKEN_KEY, trimmed);
  return true;
}

export function clearToken() {
  store().removeItem(TOKEN_KEY);
}

export function hasToken() {
  return getToken() !== null;
}

/**
 * Ask the user for a token once on an auth failure. Returns the new token or
 * null when the user cancels. Injected `promptFn` keeps this testable.
 */
export function promptForToken(promptFn) {
  const ask = promptFn || ((message) => (typeof window !== 'undefined' ? window.prompt(message) : null));
  const entered = ask('This modeld server requires an access token:');
  if (setToken(entered)) return getToken();
  return null;
}

/** Build the Authorization header value for fetches, or null when unauthenticated. */
export function authorizationHeader() {
  const token = getToken();
  return token ? 'Bearer ' + token : null;
}

/** Token for the WebSocket handshake (ws.js encodes it as a query parameter). */
export function wsToken() {
  return getToken();
}

export const SESSION_TOKEN_KEY = TOKEN_KEY;
