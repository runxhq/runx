export function webhookRequest(inputs) {
  return {
    webhook_request: {
      webhook_host: inputs.webhook_host,
      delivery: inputs.delivery,
    },
  };
}
