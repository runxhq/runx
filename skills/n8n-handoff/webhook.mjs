export function webhookRequest(inputs) {
  return {
    webhook_request: {
      webhook_host: inputs.webhook_host,
      workflow_slug: inputs.workflow_slug,
      delivery: inputs.delivery,
    },
  };
}
