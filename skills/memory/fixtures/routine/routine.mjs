// The model selects a handle. Only the admitted native memory read supplies
// the factual answer and its existing source contract; model prose is not an
// input to this materialization.
export function selected({ entry }) {
  return { answer: entry.data.text, found: true, source: entry };
}

export function missing() {
  return { answer: "No matching memory entry was selected.", found: false };
}
