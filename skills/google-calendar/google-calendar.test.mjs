import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeCalendar, combineCalendarPages } from './google-calendar.mjs';

const window = { calendar_id: 'primary', time_min: '2026-10-01T00:00:00Z', time_max: '2027-01-01T00:00:00Z' };
const event = (id = 'e1') => ({ event_id: id, revision: 'r1', status: 'confirmed', visibility: 'default', summary: 'Planning', all_day: false, start: '2026-11-04T01:00:00Z', end: '2026-11-04T02:00:00Z' });
const normalize = (result = {}, input = {}) => normalizeCalendar({
  ...window, kind: 'events', page_token: '', source_status: 'provider_readback',
  provider_result: { ...window, fetched_at: '2026-11-04T00:00:00Z', complete: true, events: [event()], ...result }, ...input,
}).calendar_context;
const pages = () => [normalize({ complete: false, next_page_token: 'next' }), normalize({ page_token: 'next', events: [event('e2')] }, { page_token: 'next' })];

test('finite multi-month windows are useful without an invented duration limit', () => {
  assert.equal(normalize().decision, 'ready');
  assert.equal(normalize().coverage, 'complete');
});
test('resuming the final page never claims the whole calendar window is complete', () => {
  const tail = pages()[1];
  assert.equal(tail.page_complete, true);
  assert.equal(tail.coverage, 'partial');
  assert.equal(tail.decision, 'partial');
  assert.equal(tail.page_token, 'next');
});
test('provider continuation is recoverable and exact requested cursor is checked', () => {
  const first = pages()[0];
  assert.equal(first.next_page_token, 'next');
  assert.equal(normalize({ page_token: 'wrong' }, { page_token: 'next' }).decision, 'blocked');
  assert.equal(normalize({ complete: false }).decision, 'blocked');
  assert.equal(normalize({ page_token: 'same', next_page_token: 'same', complete: false }, { page_token: 'same' }).decision, 'blocked');
});
test('page combination covers an exact chain and never calls a provider or invents freshness', () => {
  const chain = pages();
  chain[1].observed_at = '2026-11-04T00:10:00Z';
  const combined = combineCalendarPages({ pages: chain }).calendar_context;
  assert.equal(combined.decision, 'ready');
  assert.equal(combined.events.length, 2);
  assert.equal(combined.observed_at, chain[0].observed_at);
  assert.equal(combined.observed_until, chain[1].observed_at);
  assert.equal(combined.provider_status, 'not_called');
  assert.equal(combined.source_status, 'supplied_result');
});
test('gaps, wrong windows, reordered pages and appended pages cannot establish coverage', () => {
  assert.equal(combineCalendarPages({ pages: pages().slice(1) }).calendar_context.decision, 'blocked');
  assert.equal(combineCalendarPages({ pages: pages().reverse() }).calendar_context.decision, 'blocked');
  const wrong = pages(); wrong[1].calendar_id = 'other';
  assert.equal(combineCalendarPages({ pages: wrong }).calendar_context.decision, 'blocked');
  const extra = pages(); extra.push(extra[1]);
  assert.equal(combineCalendarPages({ pages: extra }).calendar_context.decision, 'blocked');
});
test('an unchanged duplicate is deduplicated but changed revisions are held', () => {
  const chain = pages(); chain[1].events = [Object.fromEntries(Object.entries(chain[0].events[0]).reverse())];
  const combined = combineCalendarPages({ pages: chain }).calendar_context;
  assert.equal(combined.decision, 'ready'); assert.equal(combined.events.length, 1);
  chain[1].events[0].revision = 'r2';
  assert.equal(combineCalendarPages({ pages: chain }).calendar_context.decision, 'blocked');
});
test('combination revalidates page content rather than trusting a pass boolean', () => {
  const chain = pages(); chain[1].events[0].end = '2026-11-04T00:00:00Z';
  assert.equal(combineCalendarPages({ pages: chain }).calendar_context.decision, 'blocked');
});
test('valid supplied evidence is not sliced to 2500 events or busy intervals', () => {
  assert.equal(normalize({ events: Array.from({ length: 3001 }, (_, i) => event('e'+i)) }).events.length, 3001);
  const busy = Array.from({ length: 3001 }, () => ({ start: '2026-11-04T01:00:00Z', end: '2026-11-04T02:00:00Z' }));
  const result = normalizeCalendar({ ...window, kind: 'freebusy', provider_result: { ...window, fetched_at: '2026-11-04T00:00:00Z', complete: true, errors: [], busy } }).calendar_context;
  assert.equal(result.busy.length, 3001); assert.equal(result.decision, 'ready');
});
test('freebusy preserves all error reasons and does not treat an absent busy array as free', () => {
  const base = { ...window, kind: 'freebusy' };
  const result = normalizeCalendar({ ...base, provider_result: { ...window, fetched_at: '2026-11-04T00:00:00Z', complete: false, busy: [], errors: Array.from({ length: 25 }, (_, i) => ({ reason: 'error'+i })) } }).calendar_context;
  assert.equal(result.errors.length, 25); assert.equal(result.decision, 'partial');
  assert.equal(normalizeCalendar({ ...base, provider_result: { ...window, fetched_at: '2026-11-04T00:00:00Z', complete: true, errors: [] } }).calendar_context.decision, 'blocked');
});
test('private titles, all-day exclusive ends and cancelled-instance identity survive reuse', () => {
  const result = normalize({ events: [ { ...event(), visibility: 'private', summary: 'Secret' }, { event_id: 'cancelled', status: 'cancelled', recurring_event_id: 'series' } ] });
  assert.equal('summary' in result.events[0], false);
  assert.equal(result.events[1].recurring_event_id, 'series');
  assert.equal(result.decision, 'ready');
  assert.equal(normalize({ events: [{ ...event(), all_day: true, start: '2026-02-30', end: '2026-03-03' }] }).decision, 'blocked');
});
