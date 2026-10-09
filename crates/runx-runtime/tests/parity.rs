// Test oracle: the RUNX_REGEN_FIXTURES branch prints regenerated digests to
// stderr for a human to paste back into fixtures, so the print ban is lifted.
#![allow(clippy::print_stderr)]

#[path = "parity/hello_graph.rs"]
mod hello_graph;
