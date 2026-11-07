// A minimal SCIP reader: documents, occurrences and symbol information, enough
// to walk references. Field numbers are scip.proto's.
import { readFileSync } from "node:fs";

function reader(buf) {
  let pos = 0;
  const varint = () => {
    let result = 0, shift = 0, byte;
    do {
      byte = buf[pos++];
      result += (byte & 0x7f) * 2 ** shift;
      shift += 7;
    } while (byte & 0x80);
    return result;
  };
  return {
    done: () => pos >= buf.length,
    field() {
      const tag = varint();
      return [Math.floor(tag / 8), tag & 7];
    },
    varint,
    bytes() {
      const len = varint();
      const out = buf.subarray(pos, pos + len);
      pos += len;
      return out;
    },
    skip(wire) {
      if (wire === 0) varint();
      else if (wire === 1) pos += 8;
      else if (wire === 2) {
        const len = varint();
        pos += len;
      }
      else if (wire === 5) pos += 4;
      else throw new Error(`wire type ${wire}`);
    },
  };
}

const text = (bytes) => Buffer.from(bytes).toString("utf8");
function packed(bytes) {
  const r = reader(bytes);
  const out = [];
  while (!r.done()) out.push(r.varint());
  return out;
}

function occurrence(bytes) {
  const r = reader(bytes);
  const o = { range: [], symbol: "", roles: 0, enclosing: undefined };
  while (!r.done()) {
    const [field, wire] = r.field();
    if (field === 1 && wire === 2) o.range = packed(r.bytes());
    else if (field === 2) o.symbol = text(r.bytes());
    else if (field === 3) o.roles = r.varint();
    else if (field === 7 && wire === 2) o.enclosing = packed(r.bytes());
    else r.skip(wire);
  }
  return o;
}

function relationship(bytes) {
  const r = reader(bytes);
  const rel = { symbol: "", reference: false, implementation: false, typeDefinition: false, definition: false };
  while (!r.done()) {
    const [field, wire] = r.field();
    if (field === 1) rel.symbol = text(r.bytes());
    else if (field === 2) rel.reference = !!r.varint();
    else if (field === 3) rel.implementation = !!r.varint();
    else if (field === 4) rel.typeDefinition = !!r.varint();
    else if (field === 5) rel.definition = !!r.varint();
    else r.skip(wire);
  }
  return rel;
}

function symbolInformation(bytes) {
  const r = reader(bytes);
  const s = { symbol: "", kind: 0, relationships: [], enclosing: "", name: "" };
  while (!r.done()) {
    const [field, wire] = r.field();
    if (field === 1) s.symbol = text(r.bytes());
    else if (field === 4) s.relationships.push(relationship(r.bytes()));
    else if (field === 5) s.kind = r.varint();
    else if (field === 6) s.name = text(r.bytes());
    else if (field === 8) s.enclosing = text(r.bytes());
    else r.skip(wire);
  }
  return s;
}

function document(bytes) {
  const r = reader(bytes);
  const d = { path: "", occurrences: [], symbols: [] };
  while (!r.done()) {
    const [field, wire] = r.field();
    if (field === 1) d.path = text(r.bytes());
    else if (field === 2) d.occurrences.push(occurrence(r.bytes()));
    else if (field === 3) d.symbols.push(symbolInformation(r.bytes()));
    else r.skip(wire);
  }
  return d;
}

/** Every document in the index. */
export function readIndex(file) {
  const r = reader(readFileSync(file));
  const documents = [];
  while (!r.done()) {
    const [field, wire] = r.field();
    if (field === 2 && wire === 2) documents.push(document(r.bytes()));
    else r.skip(wire);
  }
  return documents;
}
