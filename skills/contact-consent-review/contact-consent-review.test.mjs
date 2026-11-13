import assert from "node:assert/strict";
import {test} from "node:test";
import {decideListHygiene} from "./contact-consent-review.mjs";
const digest="sha256:"+"a".repeat(64);
const base={key:"contact:one",expected_version:1,idempotency_key:"consent-test-v2",engagement_evidence:{hard_bounces:0,recency_days:1},hygiene_policy:{hard_bounce_action:"suppress",decay_threshold_days:90},consent_evidence:{state:"subscribed",active_unsubscribe_marker:false,evidence_status:"read",evidence_version:1}};
const last={version:1,type:"list_hygiene.consent_transitioned",data:{type:"list_hygiene.consent_transitioned",new_state:"suppress"}};
const read={schema:"runx.store.result.v1",operation:"events.read",stream:"runx/contact-consent-review/consent",key:base.key,version:1,chain_digest:digest,events:[last]};
const run=(input=base,result=read)=>decideListHygiene({...input,contact_readback:{result}}).decision_plan;
test("a matching version cannot override recorded suppression",()=>{
 const result=run();assert.equal(result.decision.state,"human_review");assert.equal(result.reason_code,"conflicting_consent_state");assert.equal(result.observed_state,"suppress");assert.equal(result.append_allowed,false);
 const consistent=run({...base,consent_evidence:{...base.consent_evidence,state:"suppress"}});assert.equal(consistent.decision.state,"suppress");assert.equal(consistent.append_allowed,false);
});
test("unknown, missing or mismatched ledger heads cannot become clean consent",()=>{
 for(const change of [{events:[]},{events:[{...last,version:0}]},{events:[{...last,type:"other"}]},{events:[{...last,data:{...last.data,type:"other"}}]},{key:"contact:different"},{stream:"runx/contact-consent-review/other"}]){
  const result=run(base,{...read,...change});assert.equal(result.decision.state,"human_review");assert.equal(result.append_allowed,false);
 }
});
test("an empty ledger admits the original bounded observation and preserves native concurrency",()=>{
 const initial={...base,expected_version:0,consent_evidence:{...base.consent_evidence,evidence_version:0},engagement_evidence:{hard_bounces:1,recency_days:1}};
 const result=run(initial,{...read,version:0,events:[]});assert.equal(result.decision.state,"suppress");assert.equal(result.append_allowed,true);assert.equal(result.expected_version,0);
 const stale=run(initial,read);assert.equal(stale.reason_code,"stale_version");assert.equal(stale.append_allowed,false);
});
