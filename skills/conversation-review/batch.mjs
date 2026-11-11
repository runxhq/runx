// How many conversations the batch reviews at once, as many as the step's
// concurrency runs calls at once. Each lane takes the next conversation when
// its last is reviewed, so the worker holds at most this many conversations
// however many the batch holds: a batch read all at once holds more than the
// worker's 64 MiB once its conversations are large.
const LANES = 4;

// Review each conversation on its own and collect every conversation's
// outcome in the order given. Each conversation catches its own failure, so
// one conversation's failure is that conversation's outcome, with its
// reason, and never stops the rest.
export default async function batch(ctx, { conversations }) {
  const outcomes = [];
  let next = 0;
  const lane = async () => {
    while (next < conversations.length) {
      const index = next;
      next += 1;
      outcomes[index] = await review(ctx, conversations[index]);
    }
  };
  await Promise.all(Array.from({ length: LANES }, lane));
  return { outcomes };
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
