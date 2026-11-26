/**
 * The search engines runx's browser reads, in the order it tries them: each
 * one's results page for a query, the parts of the page that hold a result,
 * and the target a result's link leads to. Each page carries its results in
 * its own HTML, so the page as loaded is the page to read.
 */
export const ENGINES = [
  {
    name: "DuckDuckGo",
    url: query => "https://html.duckduckgo.com/html/?q=" + encodeURIComponent(query),
    parts: { item: ".result.web-result:not(.result--ad)", link: "a.result__a", excerpt: ".result__snippet" },
    // A result's link goes through DuckDuckGo's redirect, its target in uddg.
    target: href => new URL(href).searchParams.get("uddg"),
  },
  {
    name: "Bing",
    url: query => "https://www.bing.com/search?q=" + encodeURIComponent(query),
    parts: { item: "#b_results > li.b_algo", link: "h2 a", excerpt: ".b_caption p" },
    // A result's link goes through Bing's redirect, its target in u: "a1"
    // and the target in base64url.
    target: href => {
      const url = new URL(href);
      const wrapped = url.pathname === "/ck/a" && url.searchParams.get("u");
      if (!wrapped) return href;
      return wrapped.startsWith("a1") ? Buffer.from(wrapped.slice(2), "base64url").toString("utf8") : null;
    },
  },
  {
    name: "Mojeek",
    url: query => "https://www.mojeek.com/search?q=" + encodeURIComponent(query),
    parts: { item: "ul.results-standard > li", link: "h2 a.title", excerpt: "p.s" },
    target: href => href,
  },
];

/** The query an engine runs: the person's words, within the domains asked for. */
export function engineQuery(query, domains) {
  if (!domains?.length) return query;
  return `${query} ${domains.map(domain => "site:" + domain).join(" OR ")}`;
}

/** Whether a result's address is on one of the domains asked for. */
export function within(url, domains) {
  if (!domains?.length) return true;
  const host = url.hostname.toLowerCase();
  return domains.some(domain => host === domain.toLowerCase() || host.endsWith("." + domain.toLowerCase()));
}

// Runs in the page: each result's link, title and excerpt, as the page shows them.
export function readResults({ item, link, excerpt }) {
  const text = el => el?.textContent.replace(/\s+/g, " ").trim() ?? "";
  return [...document.querySelectorAll(item)].flatMap(entry => {
    const anchor = entry.querySelector(link);
    if (!anchor?.href) return [];
    return [{ href: anchor.href, title: text(anchor), excerpt: text(entry.querySelector(excerpt)) }];
  });
}
