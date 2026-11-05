import { test } from "node:test";
import assert from "node:assert/strict";
import { normalizeReport } from "./google-analytics.mjs";
const request = { property: "properties/123456", report_type: "standard", date_ranges: [{ start_date: "2026-10-01", end_date: "2026-10-31" }], dimensions: ["landingPage"], metrics: ["sessions"], limit: 2, offset: 0 };
const result = { property: request.property, dimension_headers: [{name: "landingPage"}], metric_headers: [{name: "sessions"}], rows: [{ dimension_values: ["/docs"], metric_values: ["50"] }], row_count: 10, pagination: { returned_rows: 1, complete: true } };
const report = (req=request, data=result) => normalizeReport({request: req, provider_result: data}).report_draft;
test("a short page cannot override a larger reported total", () => {
  const r=report(); assert.equal(r.decision,"usable_with_caveats"); assert.deepEqual(r.pagination,{returned_rows:1,complete:false,next_offset:1});
});
test("a final nonzero-offset page is partial evidence without another page", () => {
  const r=report({...request,offset:9}); assert.equal(r.decision,"usable_with_caveats"); assert.deepEqual(r.pagination,{returned_rows:1,complete:false,next_offset:null});
});
test("unknown totals remain explicit even if a connector says complete", () => {
  const {row_count,...data}=result; const r=report(request,data); assert.equal(r.pagination.complete,false); assert.equal(r.row_count,1); assert.ok(r.caveats.some(c=>c.includes("lower bound")));
});
test("inconsistent counts and skipped or stalled pages block interpretation", () => {
  for(const data of [{...result,pagination:{returned_rows:2,complete:false}},{...result,pagination:{next_offset:5,complete:false}},{...result,rows:[],pagination:{returned_rows:0,complete:false}}]) assert.equal(report(request,data).decision,"blocked");
});
test("one full result and a truly empty result retain complete coverage", () => {
  for(const data of [{...result,row_count:1},{...result,rows:[],row_count:0,pagination:{returned_rows:0,complete:true}}]) {const r=report(request,data); assert.equal(r.validation.status,"pass");assert.equal(r.pagination.complete,true);assert.equal(r.pagination.next_offset,null);}
});
