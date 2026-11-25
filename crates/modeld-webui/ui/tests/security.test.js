// tests/security.test.js
// Tests for the HTML escaping helpers used by the DOM templates.

import test from 'node:test';
import assert from 'node:assert/strict';
import { escapeHtml, safeHtml } from '../security.js';

test('escapeHtml escapes all five dangerous characters', () => {
  assert.equal(escapeHtml(`& < > " '`), '&amp; &lt; &gt; &quot; &#39;');
});

test('escapeHtml escapes the ampersand first (no double escaping)', () => {
  assert.equal(escapeHtml('&amp;'), '&amp;amp;');
  assert.equal(escapeHtml(`<a href="x">'`), '&lt;a href=&quot;x&quot;&gt;&#39;');
});

test('escapeHtml coerces arbitrary values to strings', () => {
  assert.equal(escapeHtml(null), 'null');
  assert.equal(escapeHtml(undefined), 'undefined');
  assert.equal(escapeHtml(123), '123');
  assert.equal(escapeHtml(false), 'false');
});

test('safeHtml escapes interpolations but keeps static template text', () => {
  const name = '<b>Bob</b>';
  const out = safeHtml`<div class="row" title="${name}">${name}</div>`;
  assert.equal(
    out,
    '<div class="row" title="&lt;b&gt;Bob&lt;/b&gt;">&lt;b&gt;Bob&lt;/b&gt;</div>',
  );
});

test('safeHtml returns a plain static template unchanged', () => {
  assert.equal(safeHtml`<p>Loading...</p>`, '<p>Loading...</p>');
});

test('safeHtml escapes every interpolation in a multi-value template', () => {
  const out = safeHtml`<span>${'a&b'}</span><span>${'x"y'}</span>`;
  assert.equal(out, '<span>a&amp;b</span><span>x&quot;y</span>');
});

test('a server-supplied <script> name cannot produce executable markup', () => {
  // Simulated untrusted API response.
  const serverName = '<script>alert("xss")</script>';
  const html = safeHtml`<div class="model-name">${serverName}</div>`;

  assert.ok(!html.includes('<script>'), 'raw opening script tag must be absent');
  assert.ok(!html.includes('</script>'), 'raw closing script tag must be absent');
  assert.ok(html.includes('&lt;script&gt;'), 'script tag must be escaped as text');
  assert.equal(
    html,
    '<div class="model-name">&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;</div>',
  );
});
