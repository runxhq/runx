// Start one job on the stored object, then check how it's going, waiting
// `interval_seconds` before each check, until it ends or CHECKS checks have
// found it still running. Each wait pauses the run, and a drive at or after
// the wait's time goes on from it.
const CHECKS = 20;

/// What an operation call returned.
const resultOf = (made) => made.provider_operation.result;

export default async function (ctx, { target, interval_seconds }) {
  const started = resultOf(
    await ctx.operation(
      "fixture/compute/jobs.start",
      { target },
      { idempotency_key: `compute-job:${target}` },
    ),
  );
  for (let check = 0; check < CHECKS; check += 1) {
    await ctx.wait({ until: ctx.now() + interval_seconds * 1000 });
    const job = resultOf(
      await ctx.operation("fixture/compute/jobs.inspect", { job_ref: started.job_ref }),
    );
    if (job.status === "completed" || job.status === "failed") return { job };
  }
  throw new Error(`job ${started.job_ref} was still running after ${CHECKS} checks`);
}
