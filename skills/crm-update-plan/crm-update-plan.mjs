export function finalizeUpdates({transcript, crm_records, crm_schema, update_draft, transcript_digest, records_digest}) {
  const records = new Map(crm_records.map(record => [record.id, record]));
  const allowed = new Set(crm_schema.allowed_fields);
  const seen = new Set();
  const findings = [];
  const updates = [];
  const rejected = [];
  if (records.size !== crm_records.length) findings.push({code:"records.ambiguous",message:"Record ids must identify one exact CRM snapshot."});
  for (const update of update_draft.updates) {
    const {record_id,field,to,evidence_quote} = update;
    const target = records.get(record_id);
    if (!target) {
      findings.push({code:"update.unknown_record",message:"Unknown CRM record "+record_id+"."});
      break;
    }
    if (!allowed.has(field)) {
      rejected.push({record_id,field,reason:"field is outside the crm_schema allowlist"});
      continue;
    }
    const key = JSON.stringify([record_id,field]);
    if (seen.has(key)) {
      findings.push({code:"update.conflicting_field",message:"More than one proposal targets "+record_id+"."+field+"; reconcile the intended final value."});
      break;
    }
    seen.add(key);
    if (!transcript.includes(evidence_quote)) {
      findings.push({code:"update.unsupported_evidence",message:"The quote for "+record_id+"."+field+" is absent from the transcript."});
      break;
    }
    updates.push({record_id,field,from:Object.hasOwn(target,field) ? target[field] : null,to,evidence_quote});
  }
  const failed = findings.length > 0;
  return {crm_update_proposal:{
    schema:"runx/crm-update-plan.proposal.v1",
    decision:failed ? "refused" : updates.length ? "proposed" : "no_action",
    reason:failed ? "The proposed changes do not reconcile with one unambiguous record snapshot and transcript." :
      updates.length ? "Proposed "+updates.length+" allowlisted field change(s), each traced to the transcript; no write performed." :
      rejected.length ? "No allowed changes remain; inspect the named rejected fields." : "The transcript supports no field changes.",
    updates:failed ? [] : updates,rejected_updates:rejected,write_performed:false,
    gate:"crm-operator-or-human-approver",transcript_digest,records_digest,
    validation:{status:failed ? "fail" : "pass",findings},
  }};
}
