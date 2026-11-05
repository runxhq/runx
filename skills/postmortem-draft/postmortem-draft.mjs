export function finalizePostmortem({incident_id, incident_fragments, postmortem_draft: draft, fragments_digest}) {
  const fragments = new Map(incident_fragments.map(fragment => [fragment.id, fragment]));
  const findings = [];
  if (fragments.size !== incident_fragments.length) findings.push({code:"fragments.ambiguous",message:"Fragment ids must identify one exact source."});
  const citationError = entry => {
    const fragment = fragments.get(entry.fragment_id);
    if (!fragment) return "cites an unknown fragment";
    return fragment.text.includes(entry.quote) ? null : "quotes text absent from its fragment";
  };
  for (const entry of draft.timeline) {
    const error = citationError(entry);
    if (error) {
      findings.push({code:"timeline.unsupported",message:"Timeline fragment "+entry.fragment_id+" "+error+"."});
      break;
    }
  }
  const root = draft.root_cause;
  if (root.status !== "unknown") {
    const error = citationError(root);
    if (error) findings.push({code:"root_cause.unsupported",message:"Root-cause fragment "+root.fragment_id+" "+error+"."});
  }
  const failed = findings.length > 0;
  const publishable = !failed && root.status === "known" && draft.unknowns.length === 0;
  const decision = failed ? "refused" : publishable ? "publishable" : "needs_more_evidence";
  const postmortem = {
    schema:"runx/postmortem-draft.postmortem.v1",decision,
    reason:failed ? "The draft has ambiguous or absent citation evidence." :
      publishable ? "The source-cited draft has a known root cause and no declared open unknowns; publication remains a human decision." :
      "The draft retains unresolved cause or questions; no publication is proposed.",
    summary:failed ? null : draft.summary,timeline:failed ? [] : draft.timeline,
    root_cause:failed ? null : {status:root.status,statement:root.status === "unknown" ? null : root.statement,fragment_id:root.status === "unknown" ? null : root.fragment_id},
    unknowns:draft.unknowns,action_items:failed ? [] : draft.action_items,
    publish_proposal:publishable ? {gate:"human-approver",delivery_skill:"message-send",sent:false} : null,
    publish_performed:false,fragments_digest,validation:{status:failed ? "fail" : "pass",findings},
  };
  const document = failed
    ? "# "+incident_id+"\n\nDraft withheld: "+findings.map(item => item.message).join(" ")
    : [
      "# "+incident_id,"## Summary",draft.summary,"## Timeline",
      ...draft.timeline.map(entry => "- "+entry.entry+" ["+entry.fragment_id+"]"),
      "## Root cause ("+root.status+")",root.status === "unknown" ? "Not established." : root.statement+" ["+root.fragment_id+"]",
      "## Open questions",draft.unknowns.length ? draft.unknowns.map(item => "- "+item).join("\n") : "None declared.",
      "## Actions",draft.action_items.length ? draft.action_items.map(item => "- "+item.owner+": "+item.action).join("\n") : "No actions assigned.",
      "## Source evidence",
      ...[...new Set([...draft.timeline.map(entry => entry.fragment_id),...(root.status === "unknown" ? [] : [root.fragment_id])])].map(id => {
        const fragment = fragments.get(id);
        const quotes = [...new Set([...draft.timeline.filter(entry => entry.fragment_id === id).map(entry => entry.quote),...(root.status !== "unknown" && root.fragment_id === id ? [root.quote] : [])])];
        return "["+id+"] "+fragment.source+"\n"+quotes.map(quote => "> "+quote.replace(/\n/g,"\n> ")).join("\n");
      }),
      "Publication: not performed.",
    ].join("\n\n");
  return {postmortem,document};
}
