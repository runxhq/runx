export class Refused extends Error {}

export function requireThat(condition, reason) {
  if (!condition) throw new Refused(reason);
}

export function httpUrl(value, base) {
  const url = new URL(value, base);
  requireThat(
    ["http:", "https:"].includes(url.protocol) && !url.username && !url.password,
    "The browser requires an HTTP(S) address without embedded credentials.",
  );
  return url;
}
