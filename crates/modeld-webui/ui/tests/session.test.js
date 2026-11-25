// tests/session.test.js
// The bearer token must have exactly one storage key and one read path for
// both fetch and WebSocket callers (C5).

import test, { beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  SESSION_TOKEN_KEY,
  getToken,
  setToken,
  clearToken,
  hasToken,
  authorizationHeader,
  wsToken,
  promptForToken,
  setSessionStorage,
} from '../session.js';

function memoryStorage() {
  const values = new Map();
  return {
    getItem: (key) => (values.has(key) ? values.get(key) : null),
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
  };
}

beforeEach(() => setSessionStorage(memoryStorage()));

test('session key stays stable for HTTP and WebSocket callers', () => {
  assert.equal(SESSION_TOKEN_KEY, 'modeld_token');
});

test('setToken trims the value and rejects empty input', () => {
  assert.equal(setToken('  secret-123  '), true);
  assert.equal(getToken(), 'secret-123');
  assert.equal(setToken('   '), false);
  assert.equal(getToken(), 'secret-123', 'a rejected value must not erase the old token');
});

test('clearToken and hasToken reflect the current session', () => {
  assert.equal(hasToken(), false);
  setToken('abc');
  assert.equal(hasToken(), true);
  clearToken();
  assert.equal(hasToken(), false);
  assert.equal(getToken(), null);
});

test('authorizationHeader and wsToken derive from the same stored token', () => {
  setToken('a b&c');
  assert.equal(authorizationHeader(), 'Bearer a b&c');
  assert.equal(wsToken(), 'a b&c');
  clearToken();
  assert.equal(authorizationHeader(), null);
  assert.equal(wsToken(), null);
});

test('promptForToken stores a submitted token and returns it', () => {
  let seenMessage = null;
  const returned = promptForToken((message) => {
    seenMessage = message;
    return '  typed-token  ';
  });
  assert.match(seenMessage, /access token/i);
  assert.equal(returned, 'typed-token');
  assert.equal(getToken(), 'typed-token');
});

test('promptForToken returns null when the user cancels', () => {
  const returned = promptForToken(() => null);
  assert.equal(returned, null);
  assert.equal(hasToken(), false);
});
