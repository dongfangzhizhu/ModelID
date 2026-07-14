// pages/dashboard-progress.js
//
// Pure, DOM-free helper functions for deriving the dashboard's scan progress
// bar percentage, scan-button disabled state, and phase i18n key from
// ScanProgress WsEvent payloads (audit Wave 3 / Property 8).
//
// These functions take only plain JS values in and return only plain JS
// values out, so they can be unit/property tested in plain Node without a
// browser DOM or the app's main.js/i18n.js modules (which touch
// window/localStorage at import time).
//
// Validates: Requirements 3.8, 3.9

export const PHASE_KEYS = {
  walking: 'dash.scan.walking',
  hashing: 'dash.scan.hashing',
  indexing: 'dash.scan.indexing',
  done: 'dash.scan.done',
};

/**
 * Resolve the i18n key for a given ScanProgress `phase` value.
 * Unknown/missing phases fall back to the generic "scanning" key.
 *
 * @param {string} [phase]
 * @returns {string}
 */
export function resolvePhaseKey(phase) {
  return PHASE_KEYS[phase] || 'dash.scan.scanning';
}

/**
 * Compute the progress bar percentage from a ScanProgress payload.
 * Deterministic, bounded to [0, 100], never NaN/Infinity — files_total = 0
 * (or missing/garbage fields) safely yields 0.
 *
 * @param {{files_scanned?: number, files_total?: number}} [payload]
 * @returns {number} integer percentage in [0, 100]
 */
export function computeScanProgressPercent(payload) {
  const total = Number(payload && payload.files_total) || 0;
  const scanned = Number(payload && payload.files_scanned) || 0;
  if (!(total > 0)) return 0;
  const pct = Math.round((scanned / total) * 100);
  if (!Number.isFinite(pct)) return 0;
  return Math.min(100, Math.max(0, pct));
}

/**
 * Decide whether the scan-trigger button should be disabled for a given
 * ScanProgress `phase`. The button is enabled if and only if the most
 * recently processed event has phase "done" or "error".
 *
 * @param {string} [phase]
 * @returns {boolean}
 */
export function computeScanButtonDisabled(phase) {
  return phase !== 'done' && phase !== 'error';
}
