// security.js — HTML escaping helpers for the DOM templates.
//
// Intentionally DOM-free so these functions can be unit-tested with `node --test`.

/**
 * Convert any value to a string and escape the five characters that can break
 * out of an HTML text node or a quoted attribute value: &, <, >, " and '.
 *
 * '&' is replaced first so the ampersands introduced by later replacements
 * are not escaped again.
 *
 * @param {*} value
 * @returns {string} HTML-safe text
 */
export function escapeHtml(value) {
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/**
 * Tagged template that escapes every interpolation before concatenation.
 * Static template parts are kept verbatim; only `${...}` values are escaped.
 *
 *   el.innerHTML = safeHtml`<p>${untrusted}</p>`;
 *
 * Do NOT use this when an interpolation is itself an HTML fragment built by
 * `.map(...).join('')` — the fragment would be escaped as well. In that case
 * escape the individual data fields with escapeHtml() instead.
 *
 * @param {TemplateStringsArray} strings
 * @param {...*} values
 * @returns {string}
 */
export function safeHtml(strings, ...values) {
  let out = '';
  for (let i = 0; i < strings.length; i++) {
    out += strings[i];
    if (i < values.length) out += escapeHtml(values[i]);
  }
  return out;
}
