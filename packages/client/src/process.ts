import { spawn } from "node:child_process";

export class RunxClientError extends Error {
  /** The native failure code, when runx wrote a `--json` failure envelope. */
  readonly code?: string;

  constructor(
    readonly reason: "spawn" | "timeout" | "output_limit" | "input_limit" | "aborted" | "signal" | "exit" | "contract" | "artifact",
    message: string,
    options?: ErrorOptions & { readonly code?: string },
  ) {
    super(message, options);
    this.name = "RunxClientError";
    this.code = options?.code;
  }
}

export interface ProcessOptions {
  readonly command: string;
  readonly args: readonly string[];
  readonly cwd: string;
  readonly env: NodeJS.ProcessEnv;
  readonly timeoutMs: number;
  readonly terminationGraceMs?: number;
  readonly maxOutputBytes: number;
  readonly maxInputBytes: number;
  readonly input?: string;
  readonly signal?: AbortSignal;
}

export interface ProcessResult {
  readonly exitCode: number | null;
  readonly stdout: string;
  readonly stderr: string;
}

// Private transport. Public callers choose typed native operations, never argv.
export function invoke(options: ProcessOptions): Promise<ProcessResult> {
  if (options.signal?.aborted) return Promise.reject(new RunxClientError("aborted", "Runx invocation aborted."));
  if (Buffer.byteLength(options.input ?? "") > options.maxInputBytes) {
    return Promise.reject(new RunxClientError("input_limit", `Runx input exceeds ${options.maxInputBytes} bytes.`));
  }
  return new Promise((resolve, reject) => {
    const detached = process.platform !== "win32";
    const child = spawn(options.command, [...options.args], {
      cwd: options.cwd, env: options.env, detached, stdio: ["pipe", "pipe", "pipe"],
    });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let capturedBytes = 0;
    let failure: RunxClientError | undefined;
    let escalation: NodeJS.Timeout | undefined;
    let closed = false;
    let windowsTermination: Promise<void> | undefined;
    const kill = (signal: NodeJS.Signals) => {
      try {
        if (detached && child.pid) process.kill(-child.pid, signal);
        else if (child.pid && process.platform === "win32") {
          // Windows has no POSIX process groups. Stop the tree before killing
          // the root, otherwise taskkill can no longer find its descendants.
          windowsTermination ??= new Promise<void>((done) => {
            const killer = spawn("taskkill.exe", ["/pid", String(child.pid), "/T", "/F"], {
              env: options.env, windowsHide: true, stdio: "ignore",
            });
            const deadline = setTimeout(() => { killer.kill(); child.kill(); }, 1000);
            killer.on("error", () => { child.kill(); });
            killer.on("close", (code) => {
              clearTimeout(deadline);
              if (code !== 0) child.kill();
              done();
            });
          });
        } else child.kill(signal);
      } catch (error) {
        // ESRCH also occurs when a child has already exited. Never throw from
        // a timer or stream handler; close remains the settlement boundary.
        if ((error as NodeJS.ErrnoException).code !== "ESRCH") {
          failure ??= new RunxClientError("signal", "Could not terminate Runx process group.", { cause: error });
        }
      }
    };
    const terminate = (error: RunxClientError) => {
      if (closed || failure) return;
      failure = error;
      kill("SIGTERM");
      escalation = setTimeout(() => kill("SIGKILL"), options.terminationGraceMs ?? 1000);
    };
    const abort = () => terminate(new RunxClientError("aborted", "Runx invocation aborted."));
    options.signal?.addEventListener("abort", abort, { once: true });
    const timer = setTimeout(() => terminate(new RunxClientError("timeout", `Runx invocation timed out after ${options.timeoutMs}ms.`)), options.timeoutMs);
    const capture = (target: Buffer[], chunk: Buffer) => {
      if (failure) return;
      capturedBytes += chunk.byteLength;
      if (capturedBytes > options.maxOutputBytes) {
        terminate(new RunxClientError("output_limit", `Runx output exceeds the ${options.maxOutputBytes}-byte output limit.`));
      } else target.push(chunk);
    };
    child.stdout.on("data", (chunk: Buffer) => capture(stdout, chunk));
    child.stderr.on("data", (chunk: Buffer) => capture(stderr, chunk));
    child.on("error", (error) => { failure ??= new RunxClientError("spawn", `Could not start Runx '${options.command}'.`, { cause: error }); });
    child.stdin.on("error", (error: NodeJS.ErrnoException) => {
      if (error.code !== "EPIPE") terminate(new RunxClientError("spawn", "Could not write Runx input.", { cause: error }));
    });
    child.on("close", async (exitCode, signal) => {
      closed = true;
      clearTimeout(timer);
      options.signal?.removeEventListener("abort", abort);
      if (failure && detached) {
        // The direct child may exit on SIGTERM while descendants keep running
        // after closing inherited pipes. Finish the group before returning.
        kill("SIGKILL");
      }
      if (escalation) clearTimeout(escalation);
      await windowsTermination;
      if (failure) reject(failure);
      else if (signal) reject(new RunxClientError("signal", `Runx terminated by ${signal}.`));
      else resolve({ exitCode, stdout: Buffer.concat(stdout).toString("utf8"), stderr: Buffer.concat(stderr).toString("utf8") });
    });
    child.stdin.end(options.input);
    if (options.signal?.aborted) abort();
  });
}
