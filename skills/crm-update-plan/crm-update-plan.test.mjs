import assert from "node:assert/strict";
import {test} from "node:test";
import {finalizeUpdates} from "./crm-update-plan.mjs";
const base={transcript:"Clear the secondary email.",crm_records:[{id:"northstar",email:"old@example.test"}],crm_schema:{allowed_fields:["email"]},update_draft:{updates:[{record_id:"northstar",field:"email",to:null,evidence_quote:"Clear the secondary email."}]},transcript_digest:"sha256:"+"a".repeat(64),records_digest:"sha256:"+"b".repeat(64)};
const run=(change={})=>finalizeUpdates({...base,...change}).crm_update_proposal;
test("deliberate clearing preserves before value, null and exact transcript quote",()=>{
 const result=run();assert.equal(result.decision,"proposed");assert.equal(result.updates[0].from,"old@example.test");assert.equal(result.updates[0].to,null);assert.equal(result.write_performed,false);
});
test("duplicate record identities and repeated field proposals refuse atomically",()=>{
 for(const change of [{crm_records:[...base.crm_records,{id:"northstar",email:"other@example.test"}]},{update_draft:{updates:[...base.update_draft.updates,{...base.update_draft.updates[0],to:"new@example.test"}]}}]){
  const result=run(change);assert.equal(result.decision,"refused");assert.deepEqual(result.updates,[]);
 }
});
test("disallowed fields remain explicit and unrelated valid changes remain reviewable",()=>{
 const result=run({update_draft:{updates:[...base.update_draft.updates,{record_id:"northstar",field:"owner",to:"invented",evidence_quote:"not supported"}]}});
 assert.equal(result.decision,"proposed");assert.equal(result.rejected_updates[0].field,"owner");assert.equal(result.updates.length,1);
 const wrong=run({update_draft:{updates:[{...base.update_draft.updates[0],evidence_quote:"invented"}]}});assert.equal(wrong.decision,"refused");
});
