// Explicit connector integration proof. This is not part of OSS core CI.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {mkdtempSync,mkdirSync,readFileSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {test} from 'node:test';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const binary=process.env.RUNX_BIN??path.join(root,'crates/target/debug/runx');
const skill=process.env.RUNX_PAY_SKILL??path.join(root,'skills/pay');
test('native Link payment binds session, approval, derived budget and SQLite recovery without exposing credentials',()=>{
 const sandbox=mkdtempSync(path.join(tmpdir(),'runx-link-native-'));
 try{
  const bin=path.join(sandbox,'bin');mkdirSync(bin);
  const stateFile=path.join(sandbox,'provider.json');writeFileSync(stateFile,JSON.stringify({account:'account_fixture',payments:0}));
  // The stand-in is the external provider CLI. Admission, process execution,
  // native approval, effect custody, readback and SQLite are the real runtime.
  writeFileSync(path.join(bin,'link-cli'),`#!${process.execPath}
const fs=require('node:fs'),f=${JSON.stringify(stateFile)},s=JSON.parse(fs.readFileSync(f,'utf8')),a=process.argv.slice(2);
const terms={id:s.requestId??'sr_fixture',amount:150,currency:'usd',payment_method:'pm_fixture',network_id:'profile_test_fixture',status:s.blocked?'pending_approval':'approved',credential_type:'shared_payment_token',metadata:{runx_resource_binding:'sha256:'+require('node:crypto').createHash('sha256').update(JSON.stringify(['runx:principal:link:account_fixture','https://merchant.example/invoice/42','POST','{\"invoice_id\":\"invoice-42\"}','original-public-provider-challenge'])).digest('hex')}};
let out;
if(a[0]==='--version'){console.log('0.29.0');process.exit(0);}
if(a[0]==='user-info')out={id:s.account,email:'private-identity@example.test'};
else if(a[0]==='auth')out={authenticated:true};
else if(a[0]==='spend-request')out={...terms,...(s.paidForCurrent?{link_transaction_id:'lt_fixture',payment_status_details:{outcome:'success',amount:150,currency:'usd'}}:{})};
else if(a[0]==='mpp'&&a[1]==='decode')out={method:'stripe',intent:'charge',network_id:terms.network_id,request_json:{amount:'150',currency:'usd'},expires:'2099-01-01T00:00:00Z'};
else if(a[0]==='mpp'&&a[1]==='pay'){s.payments++;s.paidForCurrent=true;fs.writeFileSync(f,JSON.stringify(s));out={status:200,body:'private-resource-body',headers:{authorization:'private-provider-token'}};}
else{process.stderr.write('unexpected external command');process.exit(1);}
console.log(JSON.stringify([out]));
`,{mode:0o755});
  const env={...process.env,PATH:bin+path.delimiter+process.env.PATH,RUNX_HOME:path.join(sandbox,'home'),RUNX_PROJECT_DIR:path.join(sandbox,'project'),LINK_TEST_PROVIDER_STATE:stateFile};
  const run=(args,input)=>{const p=spawnSync(binary,args,{cwd:root,env,input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',timeout:30000,maxBuffer:2*1024*1024});assert.ifError(p.error);let result;try{result=JSON.parse(p.stdout);}catch{assert.fail(p.stderr||p.stdout);}return result;};
  const setup=run(['connector','use','link','--session','--json']);assert.equal(setup.status,'success',JSON.stringify(setup));
  const request={action:'pay',spend_request_id:'sr_fixture',amount_minor:150,currency:'USD',payment_method_id:'pm_fixture',network_id:'profile_test_fixture',resource_url:'https://merchant.example/invoice/42',challenge:'original-public-provider-challenge',method:'POST',body:'{"invoice_id":"invoice-42"}'};
  const input={connector:'link',request,parent_payment_authority:{term_id:'invoice-42',currency:'USD',max_amount_minor:150,resource_url:request.resource_url},idempotency_seed:'native-link-payment-42'};
  const args=['skill',skill,'--run-id','link-native-proof','--inputs','-','--json'];
  const limited=run([...args,'--max-cost','0.01'],input);assert.equal(limited.status,'sealed');assert.equal(limited.outcome,'blocked');assert.equal(JSON.parse(readFileSync(stateFile)).payments,0);
  const pending=run(['skill',skill,'--run-id','link-native-allowed','--inputs','-','--max-cost','2','--json'],input);
  assert.equal(pending.status,'needs_approval',JSON.stringify(pending));assert.equal(JSON.parse(readFileSync(stateFile)).payments,0);
  const decision={approvals:Object.fromEntries(pending.requests.map(r=>[r.id,{request_digest:r.request_digest,approved:true}]))};
  const done=run(['resume',pending.run_id,'-','--json'],decision);assert.equal(done.status,'sealed',JSON.stringify(done));assert.equal(done.outcome,'completed',JSON.stringify(done));
  assert.equal(JSON.parse(readFileSync(stateFile)).payments,1);
  const output=JSON.stringify(done);assert.ok(output.includes('lt_fixture'));assert.ok(!output.includes('private-resource-body'));assert.ok(!output.includes('private-provider-token'));assert.ok(!output.includes('private-identity@example.test'));
  const replay=run(['skill',skill,'--run-id',pending.run_id,'--inputs','-','--max-cost','2','--json'],input);assert.equal(replay.receipt_id,done.receipt_id);assert.equal(JSON.parse(readFileSync(stateFile)).payments,1);
  const relabeled=run(['skill',skill,'--run-id','link-native-relabeled-resource','--inputs','-','--json'],{...input,request:{...request,resource_url:'https://merchant.example/invoice/43'},parent_payment_authority:{...input.parent_payment_authority,resource_url:'https://merchant.example/invoice/43'},idempotency_seed:'native-link-relabeled-resource'});
  assert.equal(relabeled.status,'needs_approval');
  const rejected=run(['resume',relabeled.run_id,'-','--json'],{approvals:Object.fromEntries(relabeled.requests.map(r=>[r.id,{request_digest:r.request_digest,approved:true}]))});
  assert.notEqual(rejected.outcome,'completed');assert.match(JSON.stringify(rejected),/tool_refused/);assert.doesNotMatch(JSON.stringify(rejected),/outcome_unknown/);assert.equal(JSON.parse(readFileSync(stateFile)).payments,1);
  writeFileSync(stateFile,JSON.stringify({account:'account_fixture',payments:1,requestId:'sr_pending',blocked:true,paidForCurrent:false}));
  const waitingInput={...input,request:{...request,spend_request_id:'sr_pending'},idempotency_seed:'native-link-pending-approval'};
  const waiting=run(['skill',skill,'--run-id','link-native-wallet-wait','--inputs','-','--json'],waitingInput);
  assert.equal(waiting.status,'needs_approval');
  const stopped=run(['resume',waiting.run_id,'-','--json'],{approvals:Object.fromEntries(waiting.requests.map(r=>[r.id,{request_digest:r.request_digest,approved:true}]))});
  assert.match(JSON.stringify(stopped),/tool_refused/);assert.doesNotMatch(JSON.stringify(stopped),/outcome_unknown/);assert.equal(JSON.parse(readFileSync(stateFile)).payments,1);
  writeFileSync(stateFile,JSON.stringify({account:'account_fixture',payments:1,requestId:'sr_pending',blocked:false,paidForCurrent:false}));
  const retry=run(['skill',skill,'--run-id','link-native-wallet-approved','--inputs','-','--json'],waitingInput);
  assert.equal(retry.status,'needs_approval');
  const paid=run(['resume',retry.run_id,'-','--json'],{approvals:Object.fromEntries(retry.requests.map(r=>[r.id,{request_digest:r.request_digest,approved:true}]))});
  assert.equal(paid.outcome,'completed',JSON.stringify(paid));assert.equal(JSON.parse(readFileSync(stateFile)).payments,2);
  const changed=run(['skill',skill,'--run-id','link-native-session-change','--inputs','-','--json'],{...input,idempotency_seed:'native-link-account-change'});assert.equal(changed.status,'needs_approval');
  writeFileSync(stateFile,JSON.stringify({account:'another_account',payments:2,paidForCurrent:true}));
  const refused=run(['resume',changed.run_id,'-','--json'],{approvals:Object.fromEntries(changed.requests.map(r=>[r.id,{request_digest:r.request_digest,approved:true}]))});assert.notEqual(refused.outcome,'completed');assert.equal(JSON.parse(readFileSync(stateFile)).payments,2);
 }finally{rmSync(sandbox,{recursive:true,force:true});}
});
