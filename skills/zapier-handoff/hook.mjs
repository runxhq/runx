export function hookRequest(inputs) {
  return {
    hook_request: {
      delivery: inputs.delivery,
    },
  };
}
