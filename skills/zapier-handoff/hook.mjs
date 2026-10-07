export function hookRequest(inputs) {
  return {
    hook_request: {
      zapier_account_id: inputs.zapier_account_id,
      hook_id: inputs.hook_id,
      delivery: inputs.delivery,
    },
  };
}
