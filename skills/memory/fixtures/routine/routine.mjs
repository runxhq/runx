// Source selection is judgment; date comparison and history traversal use
// admitted memory reads. The recorded clock supplies a current query's date.
export async function selected(ctx, { handle, as_of }) {
  const date = as_of || new Date(ctx.now()).toISOString().slice(0, 10);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date) || new Date(date).toISOString().slice(0, 10) !== date) {
    throw new Error("as_of must be a calendar date or empty for the current date");
  }
  const match = /^(memory:[^#]+#[^@]+)@([0-9]+)$/.exec(handle);
  if (!match || !Number.isSafeInteger(Number(match[2]))) throw new Error("select an actual memory handle");
  for (let version = Number(match[2]); version > 0; version -= 1) {
    const result = await ctx.skill("../..#read", { handle: `${match[1]}@${version}` });
    const entry = await ctx.read(result.entry);
    if (entry.data.type === "memory.disabled") return missing();
    if (entry.data.date > date) continue;
    if (entry.data.valid_until && entry.data.valid_until < date) return missing();
    return { answer: entry.data.text, found: true, source: entry };
  }
  return missing();
}

export function missing() {
  return { answer: "No matching memory entry was selected.", found: false };
}

// The model identifies facts, but cannot invent the words, confirmation,
// source or write result. Observation matching and replacement remain memory's.
export async function capture(ctx, { question, observations }) {
  const today = new Date(ctx.now()).toISOString().slice(0, 10);
  for (const { quote } of observations) {
    if (!question.includes(quote) || quote.trim().endsWith("?")) {
      throw new Error("capture an exact factual statement, never the accompanying question");
    }
  }
  for (const observation of observations) {
    const { quote, date, supersedes } = observation;
    const kept = await ctx.skill("../..#observe", {
      level: "personal", text: quote, summary: quote.replace(/[\r\n]+/g, " ").slice(0, 120),
      date: date ?? today, sources: ["conversation"], supersedes,
    });
    if (!kept.handle) throw new Error("memory did not confirm the observation write");
  }
  return { count: observations.length };
}
