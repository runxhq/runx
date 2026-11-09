import type { AgentContextEnvelopeContract, ProcessListContract, ProcessOutputContract, ProjectArtifactContract, ReceiptContract, ReferenceContract, ResumeDocumentContract, RunReadContract, RunxProcessContract, StandingApprovalContract, RunxListReportContract, SkillInspectionContract, SkillOutputContract, WaitedRequestContract } from "@runxhq/contracts";

export interface RunxClientConfig {
  readonly command: string;
  readonly cwd: string;
  readonly projectDir: string;
  /** The runx home, given to runx as RUNX_HOME. HOME stays the host environment's, so the commands a run executes find the person's own tools and configuration. */
  readonly home: string;
  /** Trusted host environment; never populated from model or skill input. */
  readonly environment?: Readonly<Record<string, string | undefined>>;
  readonly timeoutMs?: number;
  readonly terminationGraceMs?: number;
  /** Combined stdout and stderr budget. */
  readonly maxOutputBytes?: number;
  readonly maxInputBytes?: number;
  readonly maxArtifactBytes?: number;
}

export interface InvocationOptions {
  readonly signal?: AbortSignal;
  /** Untrusted skill variables cannot override runtime or process controls. */
  readonly environment?: Readonly<Record<string, string>>;
}

export interface ExecutionBinding {
  readonly method: string;
  readonly packageDigest: string;
  readonly executionClosureDigest: string;
}

/** A skill package's inspection, validated against `runx.skill.inspect.v1`. */
export type SkillInspection = SkillInspectionContract;

export interface SkillInspectOptions extends InvocationOptions {
  /** Known method inputs for target readiness; no operations execute. */
  readonly inputs?: Readonly<Record<string, unknown>>;
  readonly skillPath: string;
  /** The method to inspect or run; the package's default when absent. */
  readonly method?: string;
  /** The authority a host would start the run with: inspect projects readiness under it. */
  readonly runAuthority?: RunAuthority;
}

/**
 * A trusted host's authority for one run or inspection. The client saves it
 * in the runx home, for the workspace `cwd` names, under a name its content
 * gives (`runx authority save`), and names it with `--authority`. The run
 * record binds it; a resumed run keeps it.
 */
export interface RunAuthority {
  /**
   * The hosted grant each connector's calls run under, by connector name: the
   * host's own statement of the run's instances. Admission reads each grant
   * through the host, so a host names a grant and never asserts what it covers.
   */
  readonly grants?: Readonly<Record<string, string>>;
  /** Who acts; absent, the authority only bounds the run. */
  readonly actor_ref?: ReferenceContract;
  readonly mandate_ref?: ReferenceContract;
  /**
   * One delegation per connector instance at most, each naming its
   * `connector` and holding one or more `terms` of `verbs` over `bounds`,
   * which the kernel mints against live capability evidence. A plain
   * narrowing names nobody; one to a different actor names it as its
   * `principal_ref`, beside `actor_ref` and `mandate_ref`.
   */
  readonly delegations?: readonly Readonly<Record<string, unknown>>[];
  readonly max_fanout?: number;
  readonly max_child_depth?: number;
  /**
   * The most calls the run, its steps and child runs included, may have
   * executing at once: 1 to the kernel's ceiling of 16, which applies when
   * it's absent. A step's `concurrency` only narrows it.
   */
  readonly max_concurrency?: number;
  /**
   * What the run may change: `none` refuses every change, `ask` holds every
   * write with no approval of its own for a person, and `allow`, the default,
   * holds nothing. A method or step only narrows it.
   */
  readonly effects?: "none" | "ask" | "allow";
  /**
   * Where the run's file, git, command and authoring tools may go:
   * `workspace` keeps them inside the workspace and off every secret file and
   * runx's own state, and `inputs`, the default, reaches the roots the run's
   * inputs name. A method or step only narrows it.
   */
  readonly reach?: "workspace" | "inputs";
  /**
   * The person's standing approvals of the writes the run's approval floor
   * holds under `effects: ask`; the person replaces them through `runx resume`.
   */
  readonly standing_approvals?: readonly StandingApprovalContract[];
  /**
   * The most the run, child runs included, may spend, in integer micro-USD
   * (1 = $0.000001). A call over what is left is refused before it is sent.
   */
  readonly max_spend_micro_usd?: number;
}

export interface SkillRunOptions extends SkillInspectOptions {
  readonly inputs?: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
  /** The act the run is invoked with: why it is made. The native CLI validates it. */
  readonly act?: Readonly<Record<string, unknown>>;
  /**
   * The caller's context for the run: selected, admitted entries with their
   * provenance and sharing. Context informs the run and its children; it
   * never grants authority.
   */
  readonly context?: CallerContext;
  /**
   * The run's submission identity. Submitting it again continues that run, and
   * a sealed run returns what it sealed, so a host's retry never runs a step
   * twice.
   */
  readonly runId?: string;
}

export interface CallerContext {
  readonly current_context?: AgentContextEnvelopeContract["current_context"];
  readonly historical_context?: AgentContextEnvelopeContract["historical_context"];
}

export interface RunResumeOptions extends InvocationOptions {
  readonly runId: string;
  /** The exact `runx.resume.v1` document. The client never assigns or upgrades actors. */
  readonly resolutions: ResumeDocumentContract;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
}

export interface RunReadOptions extends InvocationOptions {
  readonly runId: string;
  readonly after?: string;
}

export interface ProcessReadOptions extends InvocationOptions {
  readonly processId: string;
  /** The `next` cursor a previous read returned; 0 by default. */
  readonly after?: number;
  /** About how much output to read. */
  readonly maxBytes?: number;
}

export interface TurnServerOptions extends InvocationOptions {
  readonly runId: string;
  readonly receiptDir: string;
}

/**
 * What an MCP stdio client starts to serve the agent turns a paused run waits
 * on (`runx mcp --run`), scoped to that run alone. Every tool call is
 * admitted, executed and recorded by the run; `turn_complete` ends the turn
 * and the run continues.
 */
export interface TurnServer {
  readonly command: string;
  readonly args: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly cwd: string;
}

/**
 * A run's signed receipt tree: the root, then every receipt its lineage
 * commits, each the one its parent names by id and signed digest.
 */
export interface ReceiptTree {
  readonly root: ReceiptContract;
  readonly descendants: readonly ReceiptContract[];
}

export interface SkillRunOutput extends SkillOutputContract {
  /** What the run waits on, each request with the digest an answer binds to. */
  readonly requests?: readonly WaitedRequestContract[];
  readonly receiptTree?: ReceiptTree;
}

export interface NativePackageOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "package";
    readonly package_digest: string;
    readonly package: Readonly<Record<string, unknown>> & { readonly markdown: string };
  };
}

/** One signed record `runx registry install --records` installed: the package it names and the install. */
export interface NativeRecordInstall extends Readonly<Record<string, unknown>> {
  readonly package: string;
  readonly install: Readonly<Record<string, unknown>> & {
    readonly status: "installed" | "unchanged";
    readonly destination: string;
    readonly skill_id: string;
    readonly version: string;
    readonly digest: string;
    readonly profile_digest?: string;
    readonly trust_tier: "first_party" | "verified" | "community";
  };
  readonly bundle_digest?: string;
}

/** `runx registry install --records - --json`: each record installed, from the registry its source names. */
export interface NativeRecordInstallOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "record_install";
    readonly source: string;
    readonly installs: readonly NativeRecordInstall[];
  };
}

/** `runx registry package <dir> --json` over a connector directory: its exact registry package. */
export interface NativeConnectorPackageOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "connector_package";
    readonly connector: Readonly<Record<string, unknown>> & {
      readonly connector: string;
      readonly version: string;
      readonly content_digest: string;
      readonly manifest_digest: string;
      readonly connector_digest: string;
      readonly manifest: Readonly<Record<string, unknown>>;
      /** Each operation's digest and its implementation's, as runx Connect binds them. */
      readonly digests: Readonly<Record<string, { readonly operation: string; readonly implementation?: string }>>;
      readonly files: readonly { readonly path: string; readonly content: string }[];
    };
  };
}

export interface NativeTestOutput {
  readonly status: "passed" | "failed" | "not_declared";
  readonly case_count: number;
  readonly assertion_error_count: number;
  readonly assertion_errors: readonly string[];
  readonly case_names: readonly string[];
  readonly receipt_ids: readonly string[];
  /** Tests whose run composed the package into a workflow. */
  readonly workflow_case_count: number;
}

export interface ReceiptVerifyOutput extends Readonly<Record<string, unknown>> {
  readonly schema: "runx.verify_verdict.v1";
  readonly valid: boolean;
  readonly receipt_id: string | null;
}

export interface RunxClient {
  readonly skills: {
    /** `runx list skills --json`: the skills this project can run. */
    list(options?: InvocationOptions): Promise<RunxListReportContract>;
    inspect(options: SkillInspectOptions): Promise<SkillInspection>;
    run(options: SkillRunOptions): Promise<SkillRunOutput>;
  };
  readonly runs: {
    /** One run's summary, waited requests and events after `after`, the cursor a previous read returned. */
    read(options: RunReadOptions): Promise<RunReadContract>;
    resume(options: RunResumeOptions): Promise<SkillRunOutput>;
  };
  readonly turns: { server(options: TurnServerOptions): TurnServer };
  /** The long-running processes the project's runs started, as the person sees them. */
  readonly processes: {
    /** Every process, or only `actor`'s, most recently changed first. */
    list(options?: InvocationOptions & { readonly actor?: string }): Promise<ProcessListContract>;
    /** What a process wrote after a cursor. */
    read(options: ProcessReadOptions): Promise<ProcessOutputContract>;
    /** Terminate it, then kill it; returns it once its exit is recorded. */
    stop(options: InvocationOptions & { readonly processId: string }): Promise<RunxProcessContract>;
  };
  readonly packages: {
    materialize(options: InvocationOptions & { readonly skillPath: string }): Promise<NativePackageOutput>;
    materializeConnector(options: InvocationOptions & { readonly connectorPath: string }): Promise<NativeConnectorPackageOutput>;
  };
  readonly registry: {
    /**
     * `runx registry install --records -`: installs signed skill version
     * records a host already holds, such as an operation's closure, each
     * verified against the keys trusted for `registry` (the runx catalog by
     * default) into the project. The records travel on stdin as one JSON
     * array, never in a file. It accepts `signal` as every operation does.
     */
    install(options: InvocationOptions & { readonly records: readonly unknown[]; readonly registry?: string }): Promise<NativeRecordInstallOutput>;
  };
  /** `runx test <path>`: runs a package's tests. */
  readonly tests: { run(options: InvocationOptions & { readonly skillPath: string; readonly receiptDir: string }): Promise<NativeTestOutput> };
  readonly artifacts: {
    /** A project artifact by its reference, confined to the project and checked against its digest. */
    read(reference: ProjectArtifactContract, options?: { readonly maxBytes?: number }): Promise<unknown>;
  };
  readonly secrets: {
    /**
     * `runx secret read <name>`: the value of a secret the operator keeps in
     * runx, for a host that sends it itself, such as a model key. runx reads
     * it now, sealed in its home or through its secrets manager; the host
     * keeps it no longer than its use.
     */
    read(options: InvocationOptions & { readonly name: string }): Promise<string>;
  };
  readonly receipts: {
    /**
     * `trust: "home"` (the default) verifies against the selected runx home's
     * own key and every configured key; `"configured"` against configured
     * keys only, as a host verifying receipts it did not issue does.
     */
    verify(options: InvocationOptions & { readonly receipt: string; readonly trust?: "home" | "configured" }): Promise<ReceiptVerifyOutput>;
  };
}
