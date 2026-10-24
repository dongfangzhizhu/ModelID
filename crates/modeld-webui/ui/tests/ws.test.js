// tests/ws.test.js
//
// Unit tests for the pure WebSocket helpers in ui/ws.js: URL construction
// (token in the query string) and the reconnect backoff schedule.

import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { wsUrl, backoffDelay, WS_MAX_BACKOFF_MS } from '../ws.js';

const HTTP_LOC = { protocol: 'http:', host: '127.0.0.1:8234' };
const HTTPS_LOC = { protocol: 'https:', host: 'models.example:443' };

test('wsUrl: no token means no query string', () => {
  assert.equal(wsUrl(HTTP_LOC, null), 'ws://127.0.0.1:8234/ws');
  assert.equal(wsUrl(HTTP_LOC, undefined), 'ws://127.0.0.1:8234/ws');
  assert.equal(wsUrl(HTTP_LOC, ''), 'ws://127.0.0.1:8234/ws');
});

test('wsUrl: token is attached and percent-encoded', () => {
  assert.equal(wsUrl(HTTP_LOC, 'abc'), 'ws://127.0.0.1:8234/ws?token=abc');
  assert.equal(wsUrl(HTTP_LOC, 'a b&c=d'), 'ws://127.0.0.1:8234/ws?token=a%20b%26c%3Dd');
});

test('wsUrl: https upgrades to wss', () => {
  assert.equal(wsUrl(HTTPS_LOC, 't'), 'wss://models.example:443/ws?token=t');
});

test('wsUrl: a round-trippable token survives encoding', () => {
  fc.assert(
    fc.property(fc.string({ minLength: 1, maxLength: 64 }), (token) => {
      const url = wsUrl(HTTP_LOC, token);
      const query = url.slice(url.indexOf('?') + 1);
      assert.equal(new URLSearchParams(query).get('token'), token);
    }),
  );
});

test('backoffDelay: doubles then caps', () => {
  assert.deepEqual(
    [1, 2, 3, 4, 5, 6, 10, 100].map(backoffDelay),
    [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000],
  );
  assert.equal(backoffDelay(0), 1000);
  assert.equal(backoffDelay(-5), 1000);
  assert.equal(backoffDelay(NaN), 1000);
});

test('backoffDelay: never exceeds the cap and never decreases', () => {
  fc.assert(
    fc.property(fc.integer({ min: 1, max: 500 }), (attempts) => {
      const delay = backoffDelay(attempts);
      assert.ok(delay <= WS_MAX_BACKOFF_MS, 'delay must stay under the cap');
      assert.ok(delay >= 1000, 'delay must be at least one second');
      assert.ok(delay >= backoffDelay(attempts - 1) || attempts > 5, 'delay must not shrink early');
    }),
  );
});
