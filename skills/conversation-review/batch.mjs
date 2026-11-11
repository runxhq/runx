// Review each conversation on its own, all at once up to the step's
// concurrency, and collect every conversation's outcome in the order given.
// Each conversation catches its own failure, so one conversation's failure
// is that conversation's outcome, with its reason, and never stops the rest.
export default async function batch(ctx, { conversations }) {
  return { outcomes: await Promise.all(conversations.map((conversation) => review(ctx, conversation))) };
}

// A batch past 64 KiB reaches the script item by item, each conversation a
// reference that ctx.read reads; under it, ctx.read gives the conversation
// back as it is.
async function review(ctx, conversation) {
  const { source_locator, reviewed } = started(ctx, await ctx.read(conversation));
  try {
    // A review packet is at most 48,379 bytes, under the 64 KiB a call's
    // result field holds inline, so it always arrives as its value.
    const { conversation_packet } = await reviewed;
    return { source_locator, status: "fulfilled", conversation_packet };
  } catch ({ reason, code, message }) {
    return { source_locator, status: "rejected", rejection: { reason, code, message } };
  }
}

// The conversation's review, started, and the one part of it the outcome
// keeps, so the worker holds no conversation once its review is called.
function started(ctx, conversation) {
  return { source_locator: conversation.source_locator, reviewed: ctx.skill(".#review", conversation) };
}
