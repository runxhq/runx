import {
  field,
  isMainModule,
  publicOutboxSummary,
  readOutboxState,
  requireObject,
  runCli,
} from "./common.mjs";

async function handler(envelope) {
  const target = requireObject(field(envelope, "publish_target") ?? {}, "publish_target");
  return { outbox: publicOutboxSummary(readOutboxState(target)) };
}

if (isMainModule(import.meta.url)) {
  runCli(handler);
}

export { handler as readOutbox };
