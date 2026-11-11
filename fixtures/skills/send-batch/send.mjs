// Send `body` to each recipient, each under its own key, and report how each
// went. A recipient whose send fails doesn't stop the others.
export default async function (ctx, { recipients, body }) {
  const settled = await Promise.allSettled(
    recipients.map((to) =>
      ctx.operation(
        "fixture/mail/message.send",
        { to, body },
        { idempotency_key: `send-batch:${to}` },
      ),
    ),
  );
  return {
    sent: settled.map((outcome, index) => {
      const to = recipients[index];
      if (outcome.status === "rejected") return { to, status: "failed", code: outcome.reason.code };
      const made = outcome.value.provider_operation;
      if (made.already_done) {
        return { to, status: "already_done", plan_digest: made.already_done.plan_digest };
      }
      return { to, status: "sent", message_ref: made.result.message_ref };
    }),
  };
}
