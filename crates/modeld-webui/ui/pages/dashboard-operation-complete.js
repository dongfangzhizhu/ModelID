// pages/dashboard-operation-complete.js
//
// Pure, DOM-free helper function for deriving which button should be
// re-enabled when an OperationComplete WsEvent arrives (audit Wave 3 /
// Property 9).
//
// This function takes only a plain JS `operation` string and returns
// 'scan' | 'gc' | null, so it can be unit/property tested in plain Node
// without a browser DOM.
//
// Validates: Requirements 3.10

/**
 * Decide which button to restore based on the `operation` field of an
 * OperationComplete event.
 *
 * - If `operation === "gc"`, return 'gc' (indicating btn-gc should be enabled)
 * - If `operation` starts with "scan", return 'scan' (indicating btn-scan should be enabled)
 * - Otherwise, return null (no button restoration)
 *
 * @param {string} [operation] - The operation type from OperationComplete payload
 * @returns {'scan' | 'gc' | null}
 */
export function decideButtonToRestore(operation) {
  if (!operation || typeof operation !== 'string') {
    return null;
  }
  
  if (operation === 'gc') {
    return 'gc';
  }
  
  if (operation.startsWith('scan')) {
    return 'scan';
  }
  
  return null;
}
