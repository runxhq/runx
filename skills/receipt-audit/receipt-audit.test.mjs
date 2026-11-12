import test from 'node:test';
import assert from 'node:assert/strict';
import finalize from './receipt-audit.mjs';
const id='receipt-test';
const evidence={matched_receipts:[{receipt_id:id}],receipt_details:[{id,verification:{status:'verified'},authority:{exercised_scopes:[{scope:'repo:read',source:'scope_ref'}]}}],chain_verification:{checked:true,intact:true,breaks:[]}};
const candidate={decision:'ready',receipt_id:id,verdict:'clean',rationale:'Inspected recorded authority.',findings:[],refusals:[],recommendations:[]};
function audit(overrides={}){return finalize({receipt_id:id,granted_scopes:['repo:read'],supplied_evidence:evidence,candidate,...overrides}).receipt_audit;}
test('scoped result binds recorded evidence without asserting usage',()=>{const result=audit();assert.equal(result.verdict,'clean');assert.deepEqual(result.exercised_scopes,['repo:read']);assert.match(result.rationale,/did not independently reread/);assert.match(result.rationale,/not prove complete or successful usage/);});
test('replay labels cannot promote an inconclusive tree to clean',()=>{const replay=structuredClone(evidence);replay.chain_verification.intact=null;assert.equal(audit({supplied_evidence:replay}).verdict,'needs_more_evidence');});
test('incomplete proof does not retain a rejected clean rationale',()=>{const replay=structuredClone(evidence);replay.chain_verification.intact=null;const result=audit({supplied_evidence:replay,candidate:{...candidate,rationale:'The receipt was verified; no evidence is missing and the audit is clean.'}});assert.equal(result.verdict,'needs_more_evidence');assert.match(result.rationale,/assessment is inconclusive/);assert(!result.rationale.includes('audit is clean'));assert(!result.rationale.includes('receipt was verified'));assert.deepEqual(result.exercised_scopes,['repo:read']);});
test('wrong receipt is held and rebound',()=>{const result=audit({candidate:{...candidate,receipt_id:'another-receipt'}});assert.equal(result.verdict,'needs_more_evidence');assert.equal(result.receipt_id,id);});
test('missing baseline and guessed wildcard coverage cannot become clean',()=>{assert.equal(audit({granted_scopes:null}).verdict,'needs_more_evidence');assert.equal(audit({granted_scopes:['repo:*']}).verdict,'needs_more_evidence');});
test('clean and an error cannot coexist',()=>{const result=audit({candidate:{...candidate,findings:[{id:'receipt.approval.inconsistent',severity:'error',message:'A recorded decision conflicts with its exact act.'}]}});assert.equal(result.verdict,'anomaly');assert(!result.findings.some(row=>row.id==='receipt.clean'));});
