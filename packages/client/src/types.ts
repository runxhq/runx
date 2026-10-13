import type { ExternalJobStageRequestContract, ReceiptContract, ReferenceContract, ResolutionRequestContract, SkillOutputContract } from "@runxhq/contracts";

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
  readonly actor_ref: ReferenceContract;
  readonly mandate_ref?: ReferenceContract;
  /** Narrowing requests the kernel mints against live capability evidence. */
  readonly delegations?: readonly Readonly<Record<string, unknown>>[];
  readonly max_fanout?: number;
  readonly max_child_depth?: number;
  /** The paid job stage the run executes. */
  readonly paid_job?: ExternalJobStageRequestContract;
}

export interface SkillRunOptions extends SkillInspectOptions {
  readonly inputs?: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
  readonly runAuthority?: RunAuthority;
}

export interface RunResumeOptions extends InvocationOptions {
  readonly runId: string;
  /** Exact native resume packet. The client never assigns or upgrades actors. */
  readonly resolutions: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
}

export interface TurnServerOptions extends InvocationOptions {
  readonly runId: string;
  readonly receiptDir: string;
}

/**
 * What an MCP stdio client starts to serve the agent turn a paused run waits
 * on. Every tool call is admitted, executed and recorded by the run;
 * `turn_complete` ends the turn and the run continues.
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
  readonly requests?: readonly ResolutionRequestContract[];
  readonly receiptTree?: ReceiptTree;
}

export interface NativePackageOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "package";
    readonly package: Readonly<Record<string, unknown>> & { readonly markdown: string };
  };
}

/** `runx registry connector package <dir> --json`: a connector's exact registry package. */
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
    inspect(options: SkillInspectOptions): Promise<SkillInspection>;
    run(options: SkillRunOptions): Promise<SkillRunOutput>;
  };
  readonly runs: { resume(options: RunResumeOptions): Promise<SkillRunOutput> };
  readonly turns: { server(options: TurnServerOptions): TurnServer };
  readonly packages: {
    materialize(options: InvocationOptions & { readonly skillPath: string }): Promise<NativePackageOutput>;
    materializeConnector(options: InvocationOptions & { readonly connectorPath: string }): Promise<NativeConnectorPackageOutput>;
  };
  readonly harness: { run(options: InvocationOptions & { readonly skillPath: string; readonly receiptDir: string }): Promise<NativeHarnessOutput> };
  readonly receipts: {
    /**
     * `trust: "home"` (the default) verifies against the selected runx home's
     * own key and every configured key; `"configured"` against configured
     * keys only, as a host verifying receipts it did not issue does.
     */
    verify(options: InvocationOptions & { readonly receipt: string; readonly trust?: "home" | "configured" }): Promise<ReceiptVerifyOutput>;
  };
}
