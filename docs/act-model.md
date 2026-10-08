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

## Skills, runs, and acts

A skill is a definition; a run is an act. The relationship is fixed:

- **Each run is an act by default.** Every governed run seals at least one act.
  When a skill declares nothing, that act is a generic `observation`: it ran, it
  was admitted by the harness, it succeeded or failed. Nothing executes
  ungoverned; the only open question is how richly the act is described. In the
  runtime this default is `RuntimeAct::observation`
  (`crates/runx-runtime/src/receipts/act.rs`).
- **A skill describes the act it performs.** A skill may declare its act in an
  `act:` block: the `ActForm` (`review`/`reply`/`revision`/`verification`/
  `observation`), the purpose, and how the target, decision, and effect are read
  from trusted, driver-pinned inputs. The runtime fills the structure from those
  inputs; the model authors only the reason prose. That trust boundary is the
  point: the skill and its inputs declare what act this is and what it targets,
  so a receipt reads "operator reviewed claim c-4417, rejected" with the model
  unable to forge the structure, only to narrate it. A declared act seals a
  domain act (`receipts::seal::domain_act_receipt`); an undeclared one seals the
  observation.
- **Acts chain.** An act records the authority it held (`Receipt.authority`,
  including the credentials it carried, as `grant_refs`) and chains by lineage:
  `lineage.previous` (this act follows the one it acts on, e.g. a review follows
  the delivery it reviewed), `lineage.children` (a graph receipt commits the
  identity and body digest of each immutable step receipt), and
  `Intent.derived_from` (the acts an act reasoned from). Runtime children do not
  carry a single `lineage.parent`: the same content-addressed receipt may be
  reused by more than one graph, so rebinding it would change its signed body.
  A graph is therefore a composition of chained acts, not a separate kind of
  thing.
- **One act per run.** The discipline that keeps a receipt honest: a run
  produces one declared (or default) act, composed into chains by lineage, never
  a loose bag of acts. A standalone skill is a one-act run; a graph is a chain
  of acts. Pausing for the host is a `needs_agent` side channel, not an act.
