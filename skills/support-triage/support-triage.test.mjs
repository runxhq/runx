import test from 'node:test';
import assert from 'node:assert/strict';
import finalize from './support-triage.mjs';

const request={source:'thread:domain-1',body:'I added the DNS records. What should I check next?'};
const documents=[{source_id:'docs:domains',text:'Open Settings > Domains and select Verify.'}];
const draft={classification:'how_to',severity:'low',recommended_path:'reply_draft',rationale:'The customer asks for a documented next step.',request_evidence:{quote:'I added the DNS records.'},product_evidence:[{source_id:'docs:domains',quote:documents[0].text}],missing_context:[],operator_notes:[],draft_email:{subject:'Next step',body:documents[0].text}};
const finish=(overrides={})=>finalize({support_request:request,product_evidence:documents,draft:{...draft,...overrides},request_digest:'request-digest',context_digest:'context-digest'}).support_triage_packet;

test('native binding preserves the selected request and product quotations',()=>{
  const result=finish();assert.equal(result.decision,'ready');assert.equal(result.delivery_status,'not_sent');
  assert.deepEqual(result.evidence,[{source_id:request.source,quote:draft.request_evidence.quote},...draft.product_evidence]);
  assert.equal(result.request_digest,'request-digest');assert.equal(result.context_digest,'context-digest');
});
for (const [name,overrides] of [
  ['missing customer selection',{request_evidence:undefined}],
  ['blank customer selection',{request_evidence:{quote:' '}}],
  ['invented customer selection',{request_evidence:{quote:'The account is verified.'}}],
  ['customer relabeled as product',{product_evidence:[{source_id:request.source,quote:request.body}]}],
  ['unknown product source',{product_evidence:[{source_id:'docs:unknown',quote:documents[0].text}]}],
]) test(name+' holds the reply without manufacturing evidence',()=>{
  const result=finish(overrides);assert.equal(result.decision,'held');assert.equal(result.draft_email,null);assert.equal(result.delivery_status,'not_sent');
});
