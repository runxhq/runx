import test from 'node:test';
import assert from 'node:assert/strict';
import {prepareReceiptQuery,finalizeReceiptQuery} from './receipt-query.mjs';

test('replay filters actor and issuer before projecting public stubs',()=>{
  const base={skill:'runx/spend',outcome:'completed',created_at:'2026-06-03T14:21:08Z',actor:'principal:ops',issuer_type:'local'};
  const inputs={question:'Bounded ops evidence',filter:{principal:'principal:ops',issuer:'local',time_range:{from:'2026-06-01T00:00:00Z',to:'2026-06-08T00:00:00Z'}},proof:{verify_chain:true},receipts:[
    {...base,receipt_id:'match'},
    {...base,receipt_id:'other-actor',actor:'principal:other'},
    {...base,receipt_id:'missing-actor',actor:undefined},
    {...base,receipt_id:'other-issuer',issuer_type:'hosted'},
    {...base,receipt_id:'missing-time',created_at:undefined},
  ]};
  const result=finalizeReceiptQuery({...inputs,...prepareReceiptQuery(inputs)});
  assert.deepEqual(result.matched_receipts.map(r=>r.receipt_id),['match']);
  assert.equal(result.chain_verification.intact,null);
  const absent=finalizeReceiptQuery({...inputs,receipts:[inputs.receipts[2]],...prepareReceiptQuery(inputs)});
  assert.equal(absent.ledger_answer.decision,'needs_more_evidence');
});
