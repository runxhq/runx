import test from 'node:test';
import assert from 'node:assert/strict';
import { prepareAssistantScan } from './assistant-source.mjs';
test('the assistant source keeps its bot exclusion independently of direct search', () => {
 const result=prepareAssistantScan({config:{query:{channel_id:'C123ABC',limit:5}},page_limit:5});
 assert.equal(result.scan_request.query.include_bots,false);
 assert.equal(result.scan_request.query.channel_id,'C123ABC');
});
test('an explicit source bot policy and exact query survive cursor continuation', () => {
 const query={channel_id:'C123ABC',mentions_external_id:'U456ABC',include_bots:true,limit:5};
 const result=prepareAssistantScan({config:{query},page_limit:5,cursor:'next'});
 assert.deepEqual(result.scan_request.query,{...query,cursor:'next'});
 assert.equal('cursor' in query,false);
});
