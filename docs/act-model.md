# Act model

## The model

The act model lives in `crates/runx-contracts`:

- **Act** (`src/act.rs`): a unit of governed work. Its kind is **`ActForm`**:
  `revision`, `reply`, `review`, `observation`, `verification`. An Act carries
  `criterion_bindings`, `change_request`/`change_plan` (`RevisionDetails`),
  target/source/artifact refs, and a closure.
- **Decision** (`src/decision.rs`): a governed choice, with a `Closure`
  (`ClosureDisposition`: `closed`, `deferred`, `superseded`, `declined`,
  `blocked`, `failed`, `killed`, `timed_out`).
- **Signal** (`src/signal.rs`): an inbound trigger, with a `SignalSchema` and a
  `SignalTrustLevel`.
- **Receipt** (`src/receipt.rs`): the sealed, flat record of a run. It inlines
  `signals: Vec<Reference>`, `decisions: Vec<Decision>`, `acts: Vec<ReceiptAct>`,
  a `seal`, `authority`, `subject`, `idempotency`, and optional `lineage`. The
  journal is a *projection* computed on demand from the receipt
  (`runx-runtime/src/journal.rs::project_receipt_journal`), not a separately
  persisted artifact.

## Calls and acts

Every governed call seals its own receipt, and the act sits in that receipt.

- **A call is an act by default.** Every step receipt carries one act. When
  nothing describes it, that act is a generic `observation`: the step ran, it
  was admitted, it succeeded or failed. In the runtime this default is
  `RuntimeAct::open` (`crates/runx-runtime/src/receipts/act.rs`).
- **A call's caller says what act it is.** An operation call may carry an act
  (`CallAct`, `crates/runx-contracts/src/act/call.rs`): its form
  (`observation`, `reply` or `review`), purpose, reason, target, decision and
  the receipt it follows. Revision and verification acts carry details one
  call does not have, so a call is never one. The act is bound into the
  call's admitted intent, so its approval covers it, and it never reaches the
  provider.
- **Admission says who and under what right.** The receipt's authority holds
  the actor admission derived for the call (the capability's principal, or
  the delegated actor under a run authority), its grant, mandate and
  execution term. The act's legitimacy names that term
  (`runx:authority_term:<id>`); no author writes it.
- **The operation says what it produced.** An operation that produces
  something declares its `effect`, a locator rendered from its arguments and
  result; the act carries each one as an artifact ref of the declared kind.
- **Acts chain.** An act chains by lineage: `lineage.previous` (the receipt of
  the act this one follows, as the caller names it), `lineage.children` (a
  graph receipt commits the identity and body digest of each immutable step
  receipt), and `Intent.derived_from`. Runtime children do not carry a single
  `lineage.parent`: the same content-addressed receipt may be reused by more
  than one graph, so rebinding it would change its signed body.
- **A run is a composition of calls.** Nothing seals an act for a run as a
  whole. The operator context a run was prepared with is each act's
  `context_ref`. Pausing for the host is a `needs_agent` side channel, not an
  act.
