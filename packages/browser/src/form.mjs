import { isDeepStrictEqual } from "node:util";
import { requireThat } from "./protocol.mjs";

export async function formSubmission(control) {
  const plan = await control.evaluate((button) => {
    const form = button.form;
    if (!form || button.type !== "submit" || button.disabled) return null;
    const method = button.getAttribute("formmethod") || form.method;
    const destination = button.getAttribute("formaction") ?? form.action;
    const encoding = button.getAttribute("formenctype") || form.enctype;
    if (method.toLowerCase() !== "post" ||
        encoding !== "application/x-www-form-urlencoded" ||
        form.matches(":invalid") ||
        [...form.elements].some(el => ["file", "password"].includes(el.type)))
      return null;

    const entries = [...new FormData(form, button)];
    const terms = [];
    for (const [name, value] of entries) {
      if (name.startsWith("_") || typeof value !== "string") return null;
      const field = form.elements.namedItem(name);
      if (!field || field.type === "hidden") return null;
      terms.push([name, value]);
    }
    return { destination, terms };
  });

  requireThat(plan && plan.terms.length > 0 && plan.terms.length <= 24,
    "This action needs a valid, inspectable HTML form. Hand off other submissions.");
  requireThat(new Set(plan.terms.map(([name]) => name)).size === plan.terms.length &&
    plan.terms.every(([name, value]) => /^[a-zA-Z][a-zA-Z0-9_-]{0,63}$/.test(name) &&
      value.length <= 2048), "The form contains ambiguous or oversized terms.");
  return { destination: plan.destination, terms: Object.fromEntries(plan.terms) };
}

export function matchesSubmission(request, submission) {
  const contentType = request.headers()["content-type"]?.split(";")[0].trim();
  const body = request.postData();
  if (contentType !== "application/x-www-form-urlencoded" || !body || body.length > 32_768)
    return false;
  const entries = [...new URLSearchParams(body)];
  return entries.length === Object.keys(submission.terms).length &&
    isDeepStrictEqual(Object.fromEntries(entries), submission.terms);
}
