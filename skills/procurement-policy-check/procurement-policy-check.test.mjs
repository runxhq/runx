import assert from "node:assert/strict";
import {test} from "node:test";
import {decideProcurement} from "./procurement-policy-check.mjs";
const base={procurement_request:{payee:"vendor",amount:1,currency:"USD"},procurement_policy:{approved_payees:["vendor"],remaining_budget:0,request_cap:100,currency:"USD"},request_digest:"sha256:"+"a".repeat(64),policy_digest:"sha256:"+"b".repeat(64)};
const run=(request={},policy={})=>decideProcurement({...base,procurement_request:{...base.procurement_request,...request},procurement_policy:{...base.procurement_policy,...policy}}).procurement_approval;
test("zero budget, cap and empty allowlist are explicit policy decisions",()=>{
 assert.equal(run().refused_reason,"over_remaining_budget");
 assert.equal(run({}, {remaining_budget:100,request_cap:0}).refused_reason,"over_request_cap");
 assert.equal(run({}, {approved_payees:[]}).refused_reason,"unapproved_payee");
 assert.equal(run().validation.status,"pass");
});
test("zero threshold requires review, including a free procurement request",()=>{
 const result=run({amount:0},{request_cap:0,approval_threshold:0});assert.equal(result.decision,"needs_human");assert.equal(result.spend_executed,false);
 const free=run({amount:0},{request_cap:0});assert.equal(free.decision,"approved");assert.equal(free.ceiling_amount,0);
});
test("currency/payee mismatch and missing policy never become authority",()=>{
 assert.equal(run({currency:"EUR"}).refused_reason,"currency_mismatch");
 const input={...base,procurement_policy:{currency:"USD"}};const result=decideProcurement(input).procurement_approval;
 assert.equal(result.refused_reason,"missing_policy_authority");assert.equal(result.validation.findings.length,3);assert.equal(result.spend_executed,false);
});
