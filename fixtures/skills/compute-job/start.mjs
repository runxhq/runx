export default async function (ctx, { target }) {
  const job = await ctx.operation(
    "fixture/compute/jobs.start",
    { target },
    { idempotency_key: "compute-job-fixture" },
  );
  return { job };
}
