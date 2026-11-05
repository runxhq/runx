import test from 'node:test';
import assert from 'node:assert/strict';
import {admitGrowthEvidence} from './organic-growth.mjs';
// The native worker supplies this API; this test checks the growth view,
// not native URL parsing or provider execution.
globalThis.Runx={parseUrl(value){const url=new URL(value);return {href:url.href,origin:url.origin,protocol:url.protocol,hostname:url.hostname};}};
test('a long native page remains usable as an explicitly bounded source view',()=>{
  const digest='sha256:'+'a'.repeat(64), source='Evidence-backed product documentation. '.repeat(1000);
  const result=admitGrowthEvidence({objective:'Prioritize improvements to this page.',site_url:'https://example.com/',collection_notes:['Private measurement evidence is unavailable; no traffic conclusion is supported.'],site_evidence:[{final_url:'https://example.com/',content_digest:digest,extracted:source,provenance:{fetched_at:'2026-11-03T00:00:00Z'}}]}).growth_context;
  assert.equal(result.decision,'ready');
  assert.equal(result.sources[0].source_digest,digest);
  assert(result.evidence[0].summary.length<source.length);
  assert(source.startsWith(result.evidence[0].summary));
  assert(result.caveats.some(note=>note.includes('omitted content was not assessed')));
  assert(result.caveats.some(note=>note.includes('Private measurement evidence is unavailable')));
  assert.equal(result.evidence[0].observed_at,'2026-11-03T00:00:00Z');
});

test('the bounded view cannot split an astral Unicode character',()=>{
  const source='A'+'😀'.repeat(10_000);
  const context=admitGrowthEvidence({objective:'Review this page.',site_url:'https://example.com/',site_evidence:[{source_uri:'https://example.com/',content_digest:'sha256:'+'b'.repeat(64),extracted:source}]}).growth_context;
  assert.equal(context.decision,'ready');
  assert(source.startsWith(context.evidence[0].summary));
  assert.doesNotThrow(()=>encodeURIComponent(context.evidence[0].summary));
  assert(context.caveats.some(note=>note.includes('omitted content was not assessed')));
});
