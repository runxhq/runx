export function decideProcurement(inputs) {
  const request = inputs.procurement_request, policy = inputs.procurement_policy;
  const fields = ["approved_payees","currency","remaining_budget","request_cap"];
  const missing = fields.filter(field => policy[field] === undefined);
  const ceiling = Math.min(policy.remaining_budget ?? 0, policy.request_cap ?? 0);
  const base = {
    schema:"runx/procurement-policy-check.approval.v1",
    payee:request.payee,amount:request.amount,currency:request.currency,ceiling_amount:ceiling,
    human_lane:null,refused_reason:null,policy_ids:[],spend_executed:false,
    request_digest:inputs.request_digest,policy_digest:inputs.policy_digest,
    validation:{status:"pass",findings:[]},
  };
  if (missing.length) return {procurement_approval:{
    ...base,decision:"refused",reason:"Required procurement policy evidence is missing.",
    refused_reason:"missing_policy_authority",
    validation:{status:"fail",findings:missing.map(field=>({code:"policy.missing",message:"procurement_policy."+field+" is required."}))},
  }};
  const failed = !policy.approved_payees.includes(request.payee) ? "unapproved_payee" :
    request.currency !== policy.currency ? "currency_mismatch" :
    request.amount > policy.remaining_budget ? "over_remaining_budget" :
    request.amount > policy.request_cap ? "over_request_cap" : null;
  base.policy_ids=fields;
  if (failed) return {procurement_approval:{...base,decision:"refused",reason:"Refused: "+failed.replaceAll("_"," ")+".",refused_reason:failed}};
  if (policy.approval_threshold !== undefined && request.amount >= policy.approval_threshold) {
    return {procurement_approval:{...base,decision:"needs_human",reason:"The amount reaches the policy review threshold; no spending authority is granted.",human_lane:policy.human_lane ?? "procurement-review",policy_ids:[...fields,"approval_threshold"]}};
  }
  return {procurement_approval:{...base,decision:"approved",reason:"The supplied payee, currency and amount fit this policy/budget snapshot; no funds are reserved and no spending authority is granted."}};
}
