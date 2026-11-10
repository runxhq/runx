import validateRoute, { limitsRefusal } from "./knowledge-router.mjs";

// One proposal, and at most two revisions of a refused one. Each proposal is
// a call and, past the inline bound, a read, so the step's max_calls of 6 is
// the hard stop behind this count.
const RETRIES = 2;

// Propose a route, validate it, and send a refused proposal back with the
// validator's findings, until one validates or the retries run out; the last
// refusal is then the result. Limits that name an entry outside the catalog
// are the caller's, so they are refused before any proposal.
export default async function route(ctx, inputs) {
  const { question, available_sources, constraints, catalog_digest } = inputs;
  const refusal = limitsRefusal(inputs);
  if (refusal) return refusal;
  let feedback;
  for (let retry = 0; ; retry += 1) {
    const proposed = await ctx.skill(".#propose", {
      question,
      available_sources,
      ...(constraints === undefined ? {} : { constraints }),
      ...(feedback === undefined ? {} : { feedback }),
    });
    const route_proposal = await ctx.read(proposed.route_proposal);
    const { knowledge_route } = validateRoute({ available_sources, constraints, route_proposal, catalog_digest });
    if (knowledge_route.validation.valid || retry === RETRIES) return { knowledge_route };
    feedback = {
      proposal: route_proposal,
      reason: knowledge_route.route.rationale,
      unknown_entries: knowledge_route.validation.unknown_entries,
    };
  }
}
