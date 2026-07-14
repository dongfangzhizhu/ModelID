// tests/dashboard.progress.test.js
//
// Property-based tests for the pure functions extracted from
// pages/dashboard.js's ScanProgress handling.
//
// Feature: modeld-audit-fixes, Property 8: Dashboard progress bar and button
// state derive deterministically from ScanProgress events.
// Validates: Requirements 3.8, 3.9

import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import {
  computeScanProgressPercent,
  computeScanButtonDisabled,
  resolvePhaseKey,
} from '../pages/dashboard-progress.js';

const KNOWN_PHASES = ['walking', 'hashing', 'indexing', 'done', 'error'];

// A payload generator that mirrors realistic ScanProgress shapes: phase is
// one of the known values, files_total is a non-negative integer, and
// files_scanned is constrained to be <= files_total (the domain the real
// backend guarantees), plus we separately fuzz unconstrained/edge inputs.
const scanProgressPayloadArb = fc
  .record({
    phase: fc.constantFrom(...KNOWN_PHASES),
    files_total: fc.nat({ max: 1_000_000 }),
  })
  .chain(({ phase, files_total }) =>
    fc.record({
      phase: fc.constant(phase),
      files_total: fc.constant(files_total),
      files_scanned: fc.integer({ min: 0, max: files_total }),
    })
  );

test('Property 8: computeScanProgressPercent is always bounded between 0 and 100', () => {
  fc.assert(
    fc.property(scanProgressPayloadArb, (payload) => {
      const pct = computeScanProgressPercent(payload);
      assert.ok(Number.isInteger(pct), `pct should be an integer, got ${pct}`);
      assert.ok(pct >= 0 && pct <= 100, `pct out of bounds: ${pct}`);
    })
  );
});

test('Property 8: computeScanProgressPercent matches round(scanned/total*100)', () => {
  fc.assert(
    fc.property(scanProgressPayloadArb, (payload) => {
      const pct = computeScanProgressPercent(payload);
      if (payload.files_total > 0) {
        const expected = Math.round(
          (payload.files_scanned / payload.files_total) * 100
        );
        assert.equal(pct, expected);
      } else {
        assert.equal(pct, 0);
      }
    })
  );
});

test('Property 8: computeScanProgressPercent never produces NaN/Infinity for files_total = 0', () => {
  fc.assert(
    fc.property(fc.nat({ max: 1_000_000 }), (filesScanned) => {
      const pct = computeScanProgressPercent({
        files_scanned: filesScanned,
        files_total: 0,
      });
      assert.equal(pct, 0);
      assert.ok(Number.isFinite(pct));
    })
  );
});

test('Property 8: computeScanProgressPercent is deterministic (same input -> same output)', () => {
  fc.assert(
    fc.property(scanProgressPayloadArb, (payload) => {
      const a = computeScanProgressPercent(payload);
      const b = computeScanProgressPercent({ ...payload });
      assert.equal(a, b);
    })
  );
});

test('Property 8: computeScanProgressPercent tolerates missing/garbage fields without crashing', () => {
  fc.assert(
    fc.property(
      fc.oneof(
        fc.constant(undefined),
        fc.constant(null),
        fc.constant({}),
        fc.record({
          files_scanned: fc.oneof(fc.constant(undefined), fc.string(), fc.integer()),
          files_total: fc.oneof(fc.constant(undefined), fc.string(), fc.integer()),
        })
      ),
      (payload) => {
        const pct = computeScanProgressPercent(payload);
        assert.ok(Number.isFinite(pct));
        assert.ok(pct >= 0 && pct <= 100);
      }
    )
  );
});

test('Property 8: computeScanButtonDisabled is false iff phase is "done" or "error"', () => {
  fc.assert(
    fc.property(fc.constantFrom(...KNOWN_PHASES), (phase) => {
      const disabled = computeScanButtonDisabled(phase);
      const shouldBeEnabled = phase === 'done' || phase === 'error';
      assert.equal(disabled, !shouldBeEnabled);
    })
  );
});

test('Property 8: computeScanButtonDisabled is true for arbitrary unknown phase strings', () => {
  fc.assert(
    fc.property(
      fc.string().filter((s) => s !== 'done' && s !== 'error'),
      (phase) => {
        assert.equal(computeScanButtonDisabled(phase), true);
      }
    )
  );
});

test('resolvePhaseKey returns a known i18n key for known phases and a fallback otherwise', () => {
  fc.assert(
    fc.property(fc.string(), (phase) => {
      const key = resolvePhaseKey(phase);
      assert.equal(typeof key, 'string');
      assert.ok(key.length > 0);
    })
  );
  assert.equal(resolvePhaseKey('walking'), 'dash.scan.walking');
  assert.equal(resolvePhaseKey('hashing'), 'dash.scan.hashing');
  assert.equal(resolvePhaseKey('indexing'), 'dash.scan.indexing');
  assert.equal(resolvePhaseKey('done'), 'dash.scan.done');
  assert.equal(resolvePhaseKey('error'), 'dash.scan.scanning');
  assert.equal(resolvePhaseKey('bogus'), 'dash.scan.scanning');
  assert.equal(resolvePhaseKey(undefined), 'dash.scan.scanning');
});

test('unit: edge case files_total = 0 with files_scanned = 0 yields 0%, button disabled', () => {
  assert.equal(computeScanProgressPercent({ files_scanned: 0, files_total: 0 }), 0);
  assert.equal(computeScanButtonDisabled('walking'), true);
});

test('unit: phase "done" always enables the button regardless of counters', () => {
  fc.assert(
    fc.property(fc.nat(), fc.nat(), (scanned, total) => {
      assert.equal(computeScanButtonDisabled('done'), false);
    })
  );
});
