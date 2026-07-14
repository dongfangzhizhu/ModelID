// tests/dashboard.operationcomplete.test.js
//
// Property-based tests for the pure function extracted from
// pages/dashboard.js's OperationComplete handling.
//
// Feature: modeld-audit-fixes, Property 9: OperationComplete events restore
// the correct button state.
// **Validates: Requirements 3.10**

import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { decideButtonToRestore } from '../pages/dashboard-operation-complete.js';

// Known operation values that the backend can emit
const KNOWN_OPERATIONS = ['gc', 'scan', 'scan:some-uuid', 'dedup', 'download', 'other'];

test('Property 9: operation === "gc" always restores gc button', () => {
  fc.assert(
    fc.property(fc.constant('gc'), (operation) => {
      assert.equal(decideButtonToRestore(operation), 'gc');
    }),
    { numRuns: 100 }
  );
});

test('Property 9: operation starting with "scan" always restores scan button', () => {
  fc.assert(
    fc.property(
      fc.oneof(
        fc.constant('scan'),
        fc.string().map((suffix) => `scan:${suffix}`),
        fc.string({ minLength: 4 }).map((suffix) => `scan${suffix}`)
      ),
      (operation) => {
        assert.equal(decideButtonToRestore(operation), 'scan');
      }
    ),
    { numRuns: 100 }
  );
});

test('Property 9: operation not matching "gc" or "scan*" returns null', () => {
  fc.assert(
    fc.property(
      fc.oneof(
        fc.constant('dedup'),
        fc.constant('download'),
        fc.constant('unknown'),
        fc.constant(''),
        fc.string({ minLength: 1, maxLength: 20 }).filter((s) => s !== 'gc' && !s.startsWith('scan'))
      ),
      (operation) => {
        assert.equal(decideButtonToRestore(operation), null);
      }
    ),
    { numRuns: 100 }
  );
});

test('Property 9: null/undefined operation returns null', () => {
  assert.equal(decideButtonToRestore(null), null);
  assert.equal(decideButtonToRestore(undefined), null);
});

test('Property 9: non-string operation values return null', () => {
  fc.assert(
    fc.property(
      fc.oneof(
        fc.integer(),
        fc.float(),
        fc.boolean(),
        fc.object(),
        fc.array(fc.string())
      ),
      (operation) => {
        assert.equal(decideButtonToRestore(operation), null);
      }
    ),
    { numRuns: 100 }
  );
});

test('Property 9: function is deterministic (same input -> same output)', () => {
  fc.assert(
    fc.property(fc.string(), (operation) => {
      const a = decideButtonToRestore(operation);
      const b = decideButtonToRestore(operation);
      assert.equal(a, b);
    }),
    { numRuns: 100 }
  );
});

test('Property 9: all results are in the expected set {"scan", "gc", null}', () => {
  fc.assert(
    fc.property(fc.string(), (operation) => {
      const result = decideButtonToRestore(operation);
      assert.ok(
        result === 'scan' || result === 'gc' || result === null,
        `unexpected result: ${result}`
      );
    }),
    { numRuns: 100 }
  );
});

test('Property 9: "gc" is the only string that returns "gc"', () => {
  // Test with a sample of strings that are definitely not "gc"
  fc.assert(
    fc.property(
      fc.oneof(
        fc.constant(''),
        fc.constant('scan'),
        fc.constant('dedup'),
        fc.constant('GC'),
        fc.string({ minLength: 3, maxLength: 20 }).filter((s) => s !== 'gc')
      ),
      (operation) => {
        const result = decideButtonToRestore(operation);
        assert.notEqual(result, 'gc', `non-"gc" operation "${operation}" returned "gc"`);
      }
    ),
    { numRuns: 100 }
  );
});

test('Property 9: any string starting with "scan" returns "scan"', () => {
  fc.assert(
    fc.property(
      fc.string().map((suffix) => `scan${suffix}`),
      (operation) => {
        assert.equal(
          decideButtonToRestore(operation),
          'scan',
          `operation "${operation}" starts with "scan" but didn't return "scan"`
        );
      }
    ),
    { numRuns: 100 }
  );
});

test('Property 9: scan with various suffixes all restore scan button', () => {
  fc.assert(
    fc.property(fc.uuid(), (uuid) => {
      const operation = `scan:${uuid}`;
      assert.equal(decideButtonToRestore(operation), 'scan');
    }),
    { numRuns: 100 }
  );
});

// Unit tests for specific known cases
test('unit: operation "gc" returns "gc"', () => {
  assert.equal(decideButtonToRestore('gc'), 'gc');
});

test('unit: operation "scan" returns "scan"', () => {
  assert.equal(decideButtonToRestore('scan'), 'scan');
});

test('unit: operation "scan:abc123" returns "scan"', () => {
  assert.equal(decideButtonToRestore('scan:abc123'), 'scan');
});

test('unit: operation "scan:uuid-format" returns "scan"', () => {
  assert.equal(decideButtonToRestore('scan:550e8400-e29b-41d4-a716-446655440000'), 'scan');
});

test('unit: operation "dedup" returns null', () => {
  assert.equal(decideButtonToRestore('dedup'), null);
});

test('unit: operation "download" returns null', () => {
  assert.equal(decideButtonToRestore('download'), null);
});

test('unit: operation "unknown" returns null', () => {
  assert.equal(decideButtonToRestore('unknown'), null);
});

test('unit: empty string returns null', () => {
  assert.equal(decideButtonToRestore(''), null);
});

test('unit: whitespace-only string returns null', () => {
  assert.equal(decideButtonToRestore('   '), null);
});

test('unit: operation with "gc" prefix but not exact match returns null', () => {
  assert.equal(decideButtonToRestore('gc-preview'), null);
  assert.equal(decideButtonToRestore('gc:something'), null);
});

test('unit: operation "GC" (uppercase) returns null (case-sensitive)', () => {
  assert.equal(decideButtonToRestore('GC'), null);
});

test('unit: operation "SCAN" (uppercase) returns null (case-sensitive)', () => {
  assert.equal(decideButtonToRestore('SCAN'), null);
});
