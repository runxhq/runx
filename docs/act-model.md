# Act model

## The model

The act model lives in `crates/runx-contracts`:

- **Act** (`src/act.rs`, `src/receipt.rs`): one governed call's act, inline in
  its receipt. Its kind is **`ActForm`**: `observation`, `reply` or `review`.
  It carries its intent (purpose and legitimacy), the caller's `reason` when
  one was stated, `criterion_bindings`, target and artifact refs, and the
  operator context it ran with. Its outcome is the receipt's seal.
- **Criterion** (`src/act.rs`): a check the runtime runs and binds, from its
  own closed vocabulary: `step_outcome`, `output_contract_verified`,
  `voice_profile_applied` and `packet_schemas_verified`. A binding states the
  check's status and the references that prove it; the runtime writes no
  prose about it.
- **Decision** (`src/decision.rs`): a governed choice a call's caller states,
  naming the act that carries it out.
- **Signal** (`src/signal.rs`): an inbound observation, with a `SignalSchema`
  and a `SignalTrustLevel`. Receipts do not carry signals.
- **Receipt** (`src/receipt.rs`): the sealed, flat record of a run node. It
  inlines `decisions: Vec<Decision>` and `acts: Vec<ReceiptAct>`, a `seal`,
  `authority`, `subject` (what ran, and its `outcome_digest`: how it ended,
  the outputs it declared and its direct children's ids), `lineage`, and an
  `issuer` that names its key and the `runx_version` that signed it. A seal's
  **`RunOutcome`** is `completed`, `deferred`, `superseded`, `declined`,
  `blocked`, `failed`, `killed` or `timed_out`. When the outcome alone does
  not say why, the seal adds a **`SealReason`** from a closed vocabulary,
  such as `graph_step_failed`, `authority_denied` or
  `input_contract_invalid`. A receipt holds only what its issuer observed or
  was given, each value once, as typed facts. Its wire form is exact: every
  array is written, an absent value is omitted rather than `null`, and
  readers refuse any other form, so every verifier hashes the same bytes. Its
  `id` is the digest of its body without the id.

## Calls and acts

A receipt states what its issuer observed and what someone stated. It never
states an intent nobody gave it.

- **Only a governed call seals an act.** A step that makes no governed call
  seals its outcome and nothing more: the seal's `step_outcome` criterion,
  with the evidence and verification the runtime recorded. Its receipt has
  no act and no decision.
- **A call's caller says what act it is.** An operation call may carry an act
  (`CallAct`, `crates/runx-contracts/src/act/call.rs`): its form
  (`observation`, `reply` or `review`), purpose, reason, target, decision and
  the receipt it follows. Revision and verification acts carry details one
  call does not have, so a call is never one. The act is bound into the
  call's admitted intent, so its approval covers it, and it never reaches the
  provider. When the caller states none, the call seals an `observation`
  whose purpose is the operation's declared `description`. In the runtime
  this is `RuntimeAct::for_call` (`crates/runx-runtime/src/receipts/act.rs`).
- **A decision is stated, never inferred.** A receipt carries a decision only
  when its call's act states one, and that decision selects the act.
- **Admission says who and under what right.** The receipt's authority holds
  the actor admission derived for the call (the capability's principal, or
  the delegated actor under a run authority), its grant, mandate and
  execution term. The act's legitimacy names that term
  (`runx:authority_term:<id>`); no author writes it.
- **The operation says what it produced.** An operation that produces
  something declares its `effect`, a locator rendered from its arguments and
  result; when the call succeeds, its act carries each one as an artifact ref
  of the declared kind.
- **No output is read for a reference.** What a receipt references is
  declared or admitted: effects, approvals, agent requests and credential
  deliveries. No output field is scanned for one.
- **Acts chain.** A receipt's lineage is what runx committed:
  `lineage.children`, the identity and body digest of each immutable step
  receipt a graph receipt seals. A child never names its parent: the same
  content-addressed receipt may be reused by more than one graph, so the edge
  is the parent's. The receipt an act follows is the caller's statement,
  `acts[].previous`, recorded as given and never checked.
- **A run is a composition of calls.** Nothing seals an act for a run as a
  whole. The operator context a run was prepared with is each act's
  `context_ref`. Pausing for the host is a `needs_agent` side channel, not an
  act.
