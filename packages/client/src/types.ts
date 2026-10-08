import type { ReceiptContract, ResolutionRequestContract, SkillOutputContract } from "@runxhq/contracts";

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

export interface SkillRunOptions extends SkillInspectOptions {
  readonly inputs?: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
}

export interface RunResumeOptions extends InvocationOptions {
  readonly runId: string;
  /** Exact native resume packet. The client never assigns or upgrades actors. */
  readonly resolutions: Readonly<Record<string, unknown>>;
  readonly receiptDir: string;
  readonly binding?: ExecutionBinding;
}

export interface SkillRunOutput extends SkillOutputContract {
  readonly requests?: readonly ResolutionRequestContract[];
  readonly receipt?: ReceiptContract;
}

export interface NativePackageOutput extends Readonly<Record<string, unknown>> {
  readonly status: "success";
  readonly registry: {
    readonly action: "package";
    readonly package: Readonly<Record<string, unknown>> & { readonly markdown: string };
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
  readonly packages: { materialize(options: InvocationOptions & { readonly skillPath: string }): Promise<NativePackageOutput> };
  readonly harness: { run(options: InvocationOptions & { readonly skillPath: string; readonly receiptDir: string }): Promise<NativeHarnessOutput> };
  readonly receipts: {
    verify(options: InvocationOptions & { readonly receipt: string; readonly allowLocalDevelopmentSignatures?: boolean }): Promise<ReceiptVerifyOutput>;
  };
}
