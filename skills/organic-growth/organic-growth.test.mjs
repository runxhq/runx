import test from 'node:test';
import assert from 'node:assert/strict';
import {admitGrowthEvidence} from './organic-growth.mjs';
// The native worker supplies this API; this test checks the growth view,
// not native URL parsing or provider execution.
globalThis.Runx={parseUrl(value){const url=new URL(value);return {href:url.href,origin:url.origin,protocol:url.protocol,hostname:url.hostname,username:url.username,password:url.password};}};
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

test('search property association uses URL origins and bounded property scope',()=>{
  const source_digest='sha256:'+'c'.repeat(64);
  const admit=(property,site_url='https://example.com/')=>admitGrowthEvidence({
    objective:'Review the supplied search evidence.',site_url,
    search_evidence:{schema:'runx/google-search-console.performance_evidence.v1',decision:'ready',validation:{status:'pass'},evidence_digest:source_digest,property,rows:[]},
  }).growth_context;
  for(const property of ['https://example.com.evil/','https://example.com:8443/','https://user@example.com/','not-a-url','sc-domain:badexample.com']){
    const result=admit(property);
    assert.equal(result.decision,'needs_more_evidence',property);
    assert(result.validation.findings.some(item=>item.code==='growth.search.site_mismatch'),property);
    assert.deepEqual(result.sources,[],property);
  }
  for(const property of ['https://example.com/','https://EXAMPLE.com','https://example.com/guides/','sc-domain:example.com']){
    const result=admit(property);
    assert.equal(result.decision,'ready',property);
    assert.equal(result.sources[0].source_digest,source_digest);
  }
  assert.equal(admit('sc-domain:example.com','https://docs.example.com/').decision,'ready');
  assert.equal(admit('https://example.com/a/','https://example.com/b/').decision,'needs_more_evidence');
});
