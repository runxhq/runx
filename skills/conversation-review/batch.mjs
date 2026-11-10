// Review each conversation on its own, all at once up to the step's
// concurrency, and collect every conversation's outcome in the order given.
// Each conversation catches its own failure, so one conversation's failure
// is that conversation's outcome, with its reason, and never stops the rest.
export default async function batch(ctx, { conversations }) {
  return { outcomes: await Promise.all(conversations.map((conversation) => review(ctx, conversation))) };
}

async function review(ctx, conversation) {
  const { source_locator } = conversation;
  try {
    // A review packet is at most 48,379 bytes, under the 64 KiB a call's
    // result field holds inline, so it always arrives as its value.
    const { conversation_packet } = await ctx.skill(".#review", conversation);
    return { source_locator, status: "fulfilled", conversation_packet };
  } catch ({ reason, code, message }) {
    return { source_locator, status: "rejected", rejection: { reason, code, message } };
  }
}
