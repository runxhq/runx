import type { AgentContextEnvelopeContract, ProcessListContract, ProcessOutputContract, ProjectArtifactRefContract, ReceiptContract, ReferenceContract, ResumeDocumentContract, RunReadContract, RunxProcessContract, StandingApprovalContract, RunxListReportContract, SkillOutputContract, WaitedRequestContract } from "@runxhq/contracts";

export interface RunxClientConfig {
  readonly command: string;
  readonly cwd: string;
  readonly projectDir: string;
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
  readonly runner: string;
  readonly packageDigest: string;
  readonly executionClosureDigest: string;
}

export interface SkillInspection extends Readonly<Record<string, unknown>> {
  readonly schema: "runx.skill.inspect.v1";
  readonly status: "ok";
  readonly name: string;
  readonly package_digest: string;
  readonly runner?: Readonly<Record<string, unknown>>;
}

export interface SkillInspectOptions extends InvocationOptions {
  readonly skillPath: string;
  readonly runner?: string;
}

/**
 * A trusted host's authority for one run, as `runx skill --run-authority`
 * reads it. The run record binds it; a resumed run keeps it.
 */
export interface RunAuthority {
  /** Who acts; absent, the authority only bounds the run. */
  readonly actor_ref?: ReferenceContract;
  readonly mandate_ref?: ReferenceContract;
  /** Narrowing requests the kernel mints against live capability evidence. */
  readonly delegations?: readonly Readonly<Record<string, unknown>>[];
  readonly max_fanout?: number;
  readonly max_child_depth?: number;
  /**
   * The run is worked by a host's agent: its file, git, command and authoring
   * tools stay inside the workspace and never touch a secret file or runx's
   * own state.
   */
  readonly agent_bounds?: boolean;
  /**
   * The person's standing approvals of the local effects the run's approval
   * floor holds; the person replaces them through `runx resume`.
   */
  readonly standing_approvals?: readonly StandingApprovalContract[];
  /**
   * The most the run, child runs included, may spend, in integer micro-USD
   * (1 = $0.000001). A call over what is left is refused before it is sent.
   */
  readonly max_cost_micro?: number;
}

export interface SkillRunOptions extends SkillInspectOptions {
  readonly inputs?: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
  readonly runAuthority?: RunAuthority;
  /** The act the run is invoked with: why it is made. The native CLI validates it. */
  readonly act?: Readonly<Record<string, unknown>>;
  /**
   * The caller's context for the run: selected, admitted entries with their
   * provenance and sharing. Context informs the run and its children; it
   * never grants authority.
   */
  readonly context?: RunContext;
  /**
   * The run's submission identity. Submitting it again continues that run, and
   * a sealed run returns what it sealed, so a host's retry never runs a step
   * twice.
   */
  readonly runId?: string;
}

export interface RunContext {
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
    readonly package: Readonly<Record<string, unknown>> & { readonly markdown: string };
  };
}

/** `runx registry package <dir> --json` over a connector directory: its exact registry package. */
export interface NativeConnectorPackageOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "connector_package";
    readonly connector: Readonly<Record<string, unknown>> & {
      readonly ref: string;
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

export interface NativeHarnessOutput {
  readonly status: "passed" | "failed" | "not_declared";
  readonly case_count: number;
  readonly assertion_error_count: number;
  readonly assertion_errors: readonly string[];
  readonly case_names: readonly string[];
  readonly receipt_ids: readonly string[];
  /** `kind: graph` fixtures that proved the skill composes into a graph. */
  readonly graph_case_count: number;
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
  readonly harness: { run(options: InvocationOptions & { readonly skillPath: string; readonly receiptDir: string }): Promise<NativeHarnessOutput> };
  readonly artifacts: {
    /** A project artifact by its reference, confined to the project and checked against its digest. */
    read(reference: ProjectArtifactRefContract, options?: { readonly maxBytes?: number }): Promise<unknown>;
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
