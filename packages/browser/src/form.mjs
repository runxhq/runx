/** HTML form ownership for the controls advertised by the current snapshot. */
export async function observedForms(page, tree) {
  const refs = [...tree.matchAll(
    /^\s*- (?:button|combobox|listbox|textbox|spinbutton)\b[^\n]*?\[ref=((?:f\d{1,8})?e\d{1,8})\]/gm,
  )].map(([, ref]) => ref);
  if (!refs.length || refs.length > 64) return [];
  // Each control was in this snapshot, and a busy page answers late, not
  // never. Only a control that has gone since waits out the page's timeout.
  const controls = await Promise.all(refs.map(async ref => {
    const control = page.getByRef(ref);
    if (await control.count() !== 1) return null;
    return control.evaluate(inspectControl);
  }));
  return controls.flatMap((control, position) => {
    if (!control?.submit) return [];
    return [{
      ref: refs[position],
      ...control.submit,
      fields: controls.flatMap((entry, index) => entry?.field && entry.index === control.index
        ? [{ ref: refs[index], ...entry.field }] : []),
    }];
  });
}

// Runs in the page: the form a control belongs to, and what it holds.
function inspectControl(el) {
  if (!el.form || el.matches(":disabled") || !el.checkVisibility()) return null;
  const form = el.form;
  const index = Array.prototype.indexOf.call(document.forms, form);
  if (index < 0) return null;
  if (el.type === "submit") {
    const label = (el.getAttribute("aria-label") || el.innerText || el.value || "").trim();
    const title = (form.getAttribute("aria-label") ||
      form.getAttribute("aria-labelledby")?.split(/\s+/).map(
        id => document.getElementById(id)?.textContent,
      ).join(" ").trim() || label).slice(0, 160);
    const scope = document.forms.length === 1 ? document : form;
    const quotes = [];
    for (const p of scope.querySelectorAll("p")) {
      if (quotes.length === 24) break;
      if (!p.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) continue;
      const text = p.innerText.trim();
      if (text && text.length <= 240) quotes.push(text);
    }
    // Where the control would send its form, and how, so a caller can check
    // the form against what it means to submit before it clicks.
    const method = (el.getAttribute("formmethod") || form.method || "get").toLowerCase();
    const destination = el.getAttribute("formaction") ? new URL(el.getAttribute("formaction"), document.baseURI).href : form.action;
    return { index, submit: { label: label.slice(0, 160), title, quotes, method, destination } };
  }
  if (!el.name || ["hidden", "file", "password"].includes(el.type)) return null;
  const options = el.tagName === "SELECT"
    ? [...el.options].filter(option => !option.matches(":disabled")).slice(0, 96)
        .map(option => option.label).filter(label => label.length <= 128)
    : undefined;
  // What the field holds now: what the form would send for it.
  return { index, field: { name: el.name.slice(0, 64), type: el.type, value: String(el.value).slice(0, 2048), options } };
}
