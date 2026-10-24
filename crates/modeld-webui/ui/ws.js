// ws.js — pure helpers for the WebSocket client.
//
// Kept free of DOM/global access so they can be unit-tested with `node --test`
// (the browser only supplies `location` at the call site).

export const WS_TOKEN_KEY = 'modeld_token';
export const WS_MAX_BACKOFF_MS = 30000;

/**
 * Build the WebSocket URL.
 *
 * Browsers cannot set headers on a handshake, so the bearer token travels as a
 * query parameter. The server only accepts it on `/ws`.
 *
 * @param {{protocol: string, host: string}} location - `window.location`-shaped object
 * @param {string|null|undefined} token
 * @returns {string}
 */
export function wsUrl(location, token) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const query = token ? '?token=' + encodeURIComponent(token) : '';
  return proto + '//' + location.host + '/ws' + query;
}

/**
 * Exponential backoff: 1 s, 2 s, 4 s, 8 s, 16 s, then capped at 30 s.
 *
 * @param {number} attempts - 1-based count of consecutive failures
 * @returns {number} delay in milliseconds
 */
export function backoffDelay(attempts) {
  const n = Number.isFinite(attempts) && attempts > 0 ? Math.floor(attempts) : 1;
  // 1 s, 2 s, 4 s … capped at WS_MAX_BACKOFF_MS (2**10 keeps the exponent sane
  // for absurd attempt counts; the millisecond cap dominates long before that).
  return Math.min(WS_MAX_BACKOFF_MS, 1000 * Math.pow(2, Math.min(n, 10) - 1));
}
