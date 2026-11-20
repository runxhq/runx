const dnsTypes = { NS: 2, CNAME: 5, TXT: 16 };
const hostname = /^(?:[a-z\d](?:[a-z\d-]{0,61}[a-z\d])?\.)+[a-z\d](?:[a-z\d-]{0,61}[a-z\d])?$/;

function bare(name) {
  return typeof name === "string" ? name.toLowerCase().replace(/\.$/, "") : "";
}

function encodeTxt(value) {
  return '"' + value.replace(/["\\]/g, "\\$&") + '"';
}

export function prepare({ name, type, content, ttl }) {
  const target = type === "CNAME" ? bare(content) : content;
  if (type === "CNAME" && !hostname.test(target)) throw new Error("Supply a CNAME destination hostname.");
  if (type === "CNAME" && target === name) throw new Error("A CNAME cannot point to its own name.");
  return { record: { name, type, content: type === "TXT" ? encodeTxt(target) : target, ttl } };
}

function answers(response, name, type) {
  if (!response || response.TC !== false || response.Status !== 0 ||
      response.Question?.length !== 1 || bare(response.Question[0].name) !== name ||
      response.Question[0].type !== dnsTypes[type]) return [];
  return (response.Answer ?? []).filter(row => bare(row.name) === name && row.type === dnsTypes[type]);
}

export function checkZone({ zone, zone_id, record, delegation, child }) {
  const requireZone = zone?.success === true && zone.zone_id === zone_id && zone.status === "active";
  const zoneName = bare(zone?.zone);
  const direct = zoneName !== "" && record.name.endsWith("." + zoneName) &&
    !record.name.slice(0, -zoneName.length - 1).includes(".");
  const expected = Array.isArray(zone?.nameservers) ? zone.nameservers.map(bare).sort() : [];
  const actual = answers(delegation, zoneName, "NS").map(row => bare(row.data)).sort();
  const childQuestion = child?.Question?.[0];
  const childChecked = child?.TC === false && [0, 3].includes(child.Status) &&
    child.Question?.length === 1 && bare(childQuestion.name) === record.name && childQuestion.type === dnsTypes.NS;
  if (!requireZone || !direct || expected.length === 0 || expected.join() !== actual.join() ||
      !childChecked || answers(child, record.name, "NS").length) {
    throw new Error("The selected active Cloudflare zone must publicly serve this direct child. Check delegation and the zone ID.");
  }
  return { zone_name: zoneName };
}

export function decide({ current, record }) {
  if (current?.success !== true || !Array.isArray(current.records) || current.records.length !== current.total ||
      current.records.some(row => bare(row.name) !== record.name)) {
    throw new Error("The Cloudflare inventory did not cover the exact name.");
  }
  if (current.records.length === 0) return { decision: "create" };
  if (current.records.length === 1) {
    const row = current.records[0];
    const value = record.type === "CNAME" ? bare(row.content) : row.content;
    if (row.type === record.type && value === record.content && row.ttl === record.ttl && row.proxied === false) {
      return { decision: "unchanged" };
    }
  }
  return { decision: "conflict" };
}

export function verify({ record, response, decision }) {
  const visible = answers(response, record.name, record.type).some(row =>
    typeof row.data === "string" && (record.type === "CNAME" ? bare(row.data) : encodeTxt(row.data)) === record.content);
  return { status: decision === "conflict" ? "manual_required" : visible ? "dns_visible" : "pending_dns",
    dns_provider_status: decision === "create" ? "readback_verified" : decision ?? "unchecked",
    record, service_status: "verification_required" };
}
