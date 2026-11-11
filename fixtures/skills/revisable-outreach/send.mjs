// The domain batch owns preparation and keys; the operation owner owns
// exact approval, dispatch and reconciliation. This fixture collects results.
export default async function (ctx, { outcomes }) {
  const settled = await Promise.allSettled(outcomes.map(async (outcome) => {
    const review = outcome.outreach_review;
    if (outcome.status !== "fulfilled" || review.decision !== "ready_for_review") {
      return { lead: outcome.lead, status: "held" };
    }
    const to = review.recipient;
    const body = `${review.draft.subject}\n\n${review.draft.body}`;
    const { provider_operation: made } = await ctx.operation(
      "fixture/mail/message.send",
      { to, body },
      {
        idempotency_key: outcome.idempotency_key,
      },
    );
    return { to, status: made.already_done ? "already_done" : "sent", packet: made };
  }));
  return {
    sent: settled.map((result, index) => result.status === "fulfilled"
      ? result.value
      : { lead: outcomes[index].lead, status: "failed", code: result.reason.code, message: result.reason.message }),
  };
}
