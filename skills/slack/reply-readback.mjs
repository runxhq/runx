export function prepareReplyReadback(inputs) {
  const delivery = inputs.delivery && typeof inputs.delivery === "object" ? inputs.delivery : {};
  const result = delivery.result && typeof delivery.result === "object" ? delivery.result : {};
  const exact = inputs.text;
  if (delivery.status !== "success" || delivery.operation !== "thread.reply" ||
      typeof result.message_locator !== "string" || !result.message_locator ||
      typeof exact !== "string" || !exact.trim()) {
    throw new Error("Slack reply lacks a bound provider message for readback");
  }
  return { read_request: { ...result, expected_text: exact } };
}
