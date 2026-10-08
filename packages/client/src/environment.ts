const HOST_ENVIRONMENT_NAMES = [
  "PATH",
  "HOME",
  "TMPDIR",
  "TMP",
  "TEMP",
  "SystemRoot",
  "WINDIR",
  "COMSPEC",
  "PATHEXT",
  "ALL_PROXY",
  "CURL_CA_BUNDLE",
  "GIT_SSL_CAINFO",
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "LANG",
  "LANGUAGE",
  "LC_ALL",
  "LC_CTYPE",
  "LC_MESSAGES",
  "LOGNAME",
  "NODE_EXTRA_CA_CERTS",
  "NO_PROXY",
  "REQUESTS_CA_BUNDLE",
  "SSL_CERT_DIR",
  "SSL_CERT_FILE",
  "TERM",
  "TZ",
  "USER",
  "all_proxy",
  "http_proxy",
  "https_proxy",
  "no_proxy",
  "COLORTERM",
] as const;

/** Copy machine interoperability variables from an explicitly supplied host. */
export function hostEnvironmentFrom(source: Readonly<Record<string, string | undefined>>): Readonly<Record<string, string>> {
  return Object.fromEntries(HOST_ENVIRONMENT_NAMES.flatMap((name) => {
    const value = source[name];
    return value === undefined ? [] : [[name, value]];
  }));
}
