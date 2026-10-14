// The receiving platforms a governed handoff can go to: each validates its
// own scope, and names its receivers under its own audience prefix.
const PLATFORMS = {
  n8n: {
    handoff_scope: "orchestrator.n8n.workflow.invoke",
    audience_prefix: "n8n:workflow:",
    default_audience: "n8n:workflow:runx-governed-effect",
  },
  zapier: {
    handoff_scope: "orchestrator.zapier.workflow.invoke",
    audience_prefix: "zapier:zap:",
    default_audience: "zapier:zap:runx-governed-effect",
  },
};

/** The scope and audience the chosen platform's receiver validates. */
export function handoffTerms(inputs) {
  const platform = PLATFORMS[inputs.platform];
  if (!platform) throw new Error(`platform must be n8n or zapier, not ${JSON.stringify(inputs.platform)}`);
  const audience = inputs.handoff_audience ?? platform.default_audience;
  if (typeof audience !== "string" || !audience.startsWith(platform.audience_prefix)) {
    throw new Error(`handoff_audience for ${inputs.platform} must start with ${platform.audience_prefix}`);
  }
  return {
    handoff_terms: {
      platform: inputs.platform,
      handoff_scope: platform.handoff_scope,
      handoff_audience: audience,
    },
  };
}

/**
 * The request the chosen connector delivers: n8n names the workspace host
 * its webhook lives on; a Zapier Catch Hook URL is the whole address, held as
 * the connector's credential.
 */
export function deliveryRequest(inputs) {
  if (inputs.platform === "n8n") {
    if (typeof inputs.webhook_host !== "string" || inputs.webhook_host.length === 0) {
      throw new Error("an n8n handoff needs webhook_host, the workspace's n8n Cloud host");
    }
    return { delivery_request: { webhook_host: inputs.webhook_host, delivery: inputs.delivery } };
  }
  if (inputs.webhook_host != null) {
    throw new Error("a Zapier handoff takes no webhook_host; its Catch Hook URL is the connector's credential");
  }
  return { delivery_request: { delivery: inputs.delivery } };
}
