import assert from 'node:assert/strict';
import {test} from 'node:test';
import {admitInvoice} from './invoice.mjs';
const request={action:'pay',resource_url:'https://merchant.example/invoice/42',amount_minor:150,currency:'EUR'};
const invoice={invoice_ref:'invoice-42',amount_minor:150,currency:'EUR',payee:{payment_url:request.resource_url},connector:'link',request};
test('invoice binding preserves a non-EVM, non-USD provider request without inventing an address or FX',()=>{assert.deepEqual(admitInvoice(invoice),{payment_request:request});for(const change of [{amount_minor:151},{currency:'USD'},{payee:{payment_url:'https://other.example/invoice'}},{invoice_ref:''}])assert.throws(()=>admitInvoice({...invoice,...change}));});
