import validateRoute, { limitsRefusal } from "./knowledge-router.mjs";

// One proposal, and at most two revisions of a refused one. Each proposal is
// a call and, past the inline bound, a read, so the step's max_calls of 6 is
// the hard stop behind this count.
const RETRIES = 2;

// Propose a route, validate it, and send a refused proposal back with the
// validator's findings, until one validates or the retries run out; the last
// refusal is then the result. Limits that name an entry outside the catalog
// are the caller's, so they are refused before any proposal. A revision that
// repeats the proposal it revises ends the loop too: the same feedback would
// go back again, and a model answering it the same way would only repeat.
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
    const repeated = feedback !== undefined && canonical(route_proposal) === canonical(feedback.proposal);
    if (knowledge_route.validation.valid || retry === RETRIES || repeated) return { knowledge_route };
    feedback = {
      proposal: route_proposal,
      reason: knowledge_route.route.rationale,
      unknown_entries: knowledge_route.validation.unknown_entries,
    };
  }
}

// A value's JSON with every object's keys in order, so two proposals that say
// the same thing compare equal whatever order their fields came in.
function canonical(value) {
  if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
  if (value !== null && typeof value === "object") {
    return "{" + Object.keys(value).sort().map((key) => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}";
  }
  return JSON.stringify(value);
}
