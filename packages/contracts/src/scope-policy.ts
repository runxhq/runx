/**
 * The governed matching classes for runx scope grants.
 *
 * Scope values remain open strings so runx does not become a registry of
 * provider permissions. The class controls only whether runx interprets
 * wildcard syntax in a grant.
 */
export const RUNX_SCOPE_GRANT_POLICY = {
  exactOnly: "exact_only",
  delegated: "delegated",
  trusted: "trusted",
} as const;

export type ScopeGrantPolicy = typeof RUNX_SCOPE_GRANT_POLICY[keyof typeof RUNX_SCOPE_GRANT_POLICY];

/**
 * Return whether one granted scope covers one requested scope.
 *
 * Delegated `namespace:*` grants cover exactly one non-empty namespace
 * segment. Only trusted first-party authority may use the universal `*`
 * grant. Exact-only matching treats provider-native values as opaque strings,
 * except for the reserved bare `*`, which only trusted grants may carry.
 */
export function scopeGrantAllows(
  grantedScope: string,
  requestedScope: string,
  policy: ScopeGrantPolicy,
): boolean {
  if (grantedScope === "*") {
    return policy === RUNX_SCOPE_GRANT_POLICY.trusted;
  }
  if (grantedScope === requestedScope) {
    return true;
  }
  if (policy === RUNX_SCOPE_GRANT_POLICY.exactOnly) {
    return false;
  }
  if (!grantedScope.endsWith(":*")) {
    return false;
  }
  const prefix = grantedScope.slice(0, -1);
  if (prefix === ":" || !requestedScope.startsWith(prefix)) {
    return false;
  }
  const suffix = requestedScope.slice(prefix.length);
  return suffix.length > 0 && !suffix.includes(":");
}

export function missingGrantedScopes(
  requiredScopes: readonly string[],
  grantedScopes: readonly string[],
  policy: ScopeGrantPolicy,
): readonly string[] {
  return requiredScopes.filter(
    (required) => !grantedScopes.some((granted) => scopeGrantAllows(granted, required, policy)),
  );
}
