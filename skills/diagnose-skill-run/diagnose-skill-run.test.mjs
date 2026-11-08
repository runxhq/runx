import test from 'node:test';
import assert from 'node:assert/strict';
import finish from './diagnose-skill-run.mjs';
const draft = { verdict: 'needs_update', failure_summary: 'The caller used an undeclared input.', improvement_proposals: [{ target: 'caller inputs', change: 'Use snapshot.', rationale: 'It is the declared port.', risk: 'Pass the exact prior descriptor.' }], next_test_checks: ['The documented input reaches offline analysis.'] };
test('a confident unsupported cause is held when failure evidence is absent', () => {
  const result = finish({ receipt_id: 'missing', receipt_evidence: { receipt_details: [] }, candidate: { ...draft, evidence: ['invented:reference'] } });
  assert.equal(result.verdict, 'blocked'); assert.deepEqual(result.improvement_proposals, []);
});
test('supplied diagnostic material remains clearly caller-supplied evidence', () => {
  const result = finish({ test_output: 'input captured_comparison is not declared', candidate: draft });
  assert.equal(result.verdict, 'needs_update');
  assert.equal(result.evidence[0].source, 'supplied_test');
});
test('a receipt selection does not mix an unrelated receipt into causal support', () => {
  const result = finish({ receipt_id: 'selected', receipt_evidence: { receipt_details: [{ id: 'other', outcome: 'failed' }] }, candidate: draft });
  assert.equal(result.verdict, 'blocked');
});
test('a healthy suspension and prior verification posture survive without a repair proposal', () => {
  const result = finish({ receipt_id: 'paused', receipt_evidence: { receipt_details: [{ id: 'paused', outcome: 'deferred', seal_reason: 'resolution_pending', verification: { status: 'verified' } }] }, candidate: { verdict: 'pass', failure_summary: 'The selected graph is waiting for its caller turn.', improvement_proposals: [], next_test_checks: ['Complete the intended caller turn.'] } });
  assert.equal(result.verdict, 'pass');
  assert.match(result.evidence[0].observation, /outcome=deferred/);
  assert.match(result.evidence[0].limitation, /does not independently reverify/);
});
test('unbounded or internally contradictory repair reports are refused', () => {
  assert.throws(() => finish({ test_output: 'failure', candidate: { ...draft, improvement_proposals: Array(4).fill(draft.improvement_proposals[0]) } }), /at most three/);
  assert.throws(() => finish({ test_output: 'failure', candidate: { ...draft, verdict: 'pass' } }), /passing diagnosis/);
});
