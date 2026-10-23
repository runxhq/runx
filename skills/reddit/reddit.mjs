// Pure Reddit domain admission. Storage, digest, approval and host tools stay native.
const ensure = (ok, message) => { if (!ok) throw new Error(message); };
const norm = value => value.replace(/\r\n?/gu, '\n');
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const username = value => typeof value === 'string' && /^[a-z0-9_-]{3,20}$/u.test(value);

function tail(packet, resource, aggregate) {
  ensure(packet?.status === 'read', 'State read did not complete.');
  ensure(packet.resource === resource && packet.aggregate_id === aggregate, 'State belongs to another resource or account.');
  ensure(Number.isInteger(packet.after_version) && packet.after_version >= 0, 'Missing state version.');
  ensure(Array.isArray(packet.events), 'Missing event tail.');
  if (packet.after_version === 0) {
    ensure(packet.events.length === 0, 'Empty state has unexpected events.');
    return { version: 0, event: null };
  }
  ensure(packet.events.length === 1 && packet.events[0].version === packet.after_version, 'State tail does not match its head.');
  return { version: packet.after_version, event: packet.events[0].event };
}
function accountState(packet, accountId) {
  ensure(username(accountId), 'Select an explicit lowercase Reddit username.');
  const state = tail(packet, 'reddit_accounts', accountId);
  ensure(state.event?.type === 'reddit.account', 'Account is not configured.');
  ensure(state.event.account_id === accountId, 'Account configuration identity mismatch.');
  validateConfig(accountId, state.event.config);
  return { account_id: accountId, version: state.version, config: state.event.config };
}
function validateConfig(accountId, config) {
  ensure(username(accountId), 'Select an explicit lowercase Reddit account.');
  ensure(config?.voice && ['lowercase', 'sentence_case', 'preserve'].includes(config.voice.casing), 'Configure an explicit account voice.');
  ensure(Array.isArray(config.voice.principles) && Array.isArray(config.voice.examples), 'Voice needs principles and examples.');
}
function redditUrl(value) {
  const url = Runx.parseUrl(value);
  ensure(url.protocol === 'https:' && ['www.reddit.com', 'reddit.com', 'old.reddit.com'].includes(url.hostname), 'Target must be an HTTPS Reddit page.');
  const match = /^https:\/\/(?:www\.|old\.)?reddit\.com(\/[^?#]*)$/u.exec(value);
  ensure(match && url.href === value, 'Use a canonical Reddit URL without credentials, port, query or fragment.');
  const pathname = match[1];
  const parts = pathname.split('/').filter(Boolean);
  ensure(parts[0] === 'r' && /^[A-Za-z0-9_]{2,21}$/u.test(parts[1] || ''), 'Target must name a subreddit.');
  ensure(!/%|\\/u.test(pathname), 'Encoded or ambiguous Reddit paths are not allowed.');
  return { parts, community: parts[1].toLowerCase(), path: pathname.replace(/\/+$/u, '') };
}
function validateAction(action) {
  const target = redditUrl(action.target_url);
  ensure(target.community === action.subreddit.toLowerCase(), 'Target community differs from the approved subreddit.');
  ensure(action.body.trim().length > 0, 'Empty Reddit contribution.');
  ensure(action.kind === 'upvote' || !/\[[^\]\n]+\]\([^)]+\)/u.test(action.body), 'Reddit editor text must not contain Markdown links.');
  ensure(action.kind === 'upvote' || action.allow_bare_urls || !/https?:\/\/|www\./iu.test(action.body), 'Bare URLs require explicit inclusion in the action.');
  if (action.kind === 'post') {
    ensure(target.parts.length === 3 && target.parts[2] === 'submit', 'A post must target the exact subreddit submit page.');
    ensure(action.title.trim().length > 0, 'A post needs a title.');
  } else {
    ensure(action.kind === 'upvote' || action.title === '', 'A comment must not carry a post title.');
    ensure(target.parts[2] === 'comments' && /^[a-z0-9]+$/u.test(target.parts[3] || ''), 'A comment needs a specific thread or comment permalink.');
    ensure(target.parts.length >= 4 && target.parts.length <= 7, 'Unexpected comment target path.');
    if (target.parts.length === 7) ensure(target.parts[5] === 'comment' && /^[a-z0-9]+$/u.test(target.parts[6]), 'Invalid comment permalink.');
    if (target.parts.length === 6) ensure(/^[a-z0-9]+$/u.test(target.parts[5]), 'Invalid comment permalink.');
  }
  return action;
}
function targetIdentity(value) {
  const {parts, community} = redditUrl(value);
  return { community, thread: parts[3] || '', comment: parts[5] === 'comment' ? (parts[6] || '') : (parts[5] || '') };
}
export function configure(inputs) {
  validateConfig(inputs.account_id, inputs.config);
  ensure(username(inputs.account_id), 'Use the lowercase Reddit username as account_id.');
  return { configuration: { account_id: inputs.account_id, config: inputs.config, expected_version: inputs.expected_version }, event: { type: 'reddit.account', account_id: inputs.account_id, config: inputs.config } };
}
export function loadAccount(inputs) {
  return { account: accountState(inputs.account_state, inputs.account_id) };
}
export function validateResearch(inputs) {
  const research = inputs.research;
  let reason = '';
  let provisional = false;
  try {
    ensure(research.account_id === inputs.account.account_id, 'Research account differs from the loaded account.');
    const selected = research.target_candidates.filter(candidate => candidate.decision === 'selected');
    for (const candidate of selected) {
      const target = redditUrl(candidate.target_url);
      ensure(target.community === candidate.subreddit.toLowerCase(), 'Selected target community mismatch.');
      ensure(candidate.rule_status === 'allowed', 'Selected target needs confirmed community rules.');
      ensure(candidate.account_eligibility !== 'ineligible', 'Selected target account is ineligible.');
      if (candidate.account_eligibility === 'unknown') provisional = true;
      if (candidate.intent === 'post') ensure(target.parts.length === 3 && target.parts[2] === 'submit', 'Selected post target must be its subreddit submit page.');
      else ensure(target.parts[2] === 'comments' && /^[a-z0-9]+$/u.test(target.parts[3] || ''), 'Selected comment target must be a specific thread.');
    }
    for (const draft of research.drafts) {
      const action = validateAction(draft.action);
      ensure(action.kind === 'post' || action.kind === 'comment', 'Target discovery drafts must be posts or comments.');
      const intent = action.kind;
      ensure(selected.some(candidate => candidate.intent === intent && candidate.target_url === action.target_url), 'Draft has no selected, eligible target candidate.');
    }
  } catch (error) { reason = error.message; }
  const needsEligibility = !reason && provisional && research.drafts.length > 0;
  return {
    research: reason
      ? { ...research, account_id: inputs.account.account_id, status: 'held', drafts: [], unknowns: [...research.unknowns.slice(0, 11), reason], next_step: 'Re-evaluate the target evidence before drafting or publishing.' }
      : needsEligibility
        ? { ...research, status: 'provisional', unknowns: [...research.unknowns.slice(0, 11), 'Account eligibility is unverified.'], next_step: 'Review the draft; verify the logged-in account, community eligibility, thread, and editor immediately before any approved submission.' }
        : research,
    target_review: { admitted: reason === '', reason: needsEligibility ? 'Draft admitted provisionally; account eligibility requires an authenticated recheck.' : reason, candidate_count: research.target_candidates.length, draft_count: reason ? 0 : research.drafts.length }
  };
}
export function identify(inputs) {
  const a = inputs.action;
  let target = a.target_url;
  try { target = JSON.stringify(targetIdentity(a.target_url)); } catch { /* Admission reports invalid targets. */ }
  return { identity: { account_id: inputs.account_id, kind: a.kind, target, title: a.kind === 'upvote' ? '' : a.title, body: a.kind === 'upvote' ? '' : norm(a.body) } };
}
export function prepare(inputs) {
  try {
    const account = accountState(inputs.account_state, inputs.account_id);
    const action = validateAction(inputs.action);
    const history = tail(inputs.history, 'reddit_contributions', inputs.action_id);
    ensure(!history.event, 'This exact contribution is already recorded. Do not submit it again.');
    if (action.kind === 'upvote') {
      ensure(username(action.target_author) && action.target_author !== account.account_id, 'Do not vote on your own content.');
      ensure(tail(inputs.target_account_state, 'reddit_accounts', action.target_author).version === 0, 'Do not vote on content by another managed account.');
    }
    const operation = {post: 'post.create', comment: 'comment.create', upvote: 'vote.upvote'}[action.kind];
    const input = {
      operation: {post: 'post-create', comment: 'comment-create', upvote: 'upvote'}[action.kind],
      account: account.account_id,
      target_url: action.target_url, body: norm(action.body),
      ...(action.kind === 'post' ? {title: action.title} : {}),
      ...(action.kind === 'upvote' ? {target_author: action.target_author} : {}),
    };
    return {admission: {admitted: true, reason: '', account_id: account.account_id, action, operation, input}};
  } catch (error) {
    return {admission: {admitted: false, reason: error.message}};
  }
}
export function finish(inputs) {
  const p = inputs.provider_operation;
  ensure(p.status === 'success' && p.finality === 'confirmed' && p.readback_uri, 'The operation has no confirmed provider readback. Continue the original native run; do not submit again.');
  return {contribution: {type: 'reddit.contribution', account_id: inputs.admission.account_id,
    action: inputs.admission.action, permalink: p.result.permalink, readback_uri: p.readback_uri,
    visibility: p.result.visibility}};
}
export function refuse(inputs) {
  return {result: {status: 'refused', account_id: inputs.account_id, action: inputs.action, reason: inputs.reason}};
}
export function planRead(inputs) {
  const operation = {inspect:'account.inspect', navigate:'page.navigate', feed:'feed.read', rules:'rules.read', thread:'thread.read', composer:'composer.read'}[inputs.operation];
  ensure(operation, 'Select a declared Reddit read operation.');
  ensure(username(inputs.account_id), 'Select an explicit lowercase account.');
  if (inputs.operation === 'navigate') ensure(typeof inputs.url === 'string', 'Navigation requires an exact Reddit URL.');
  return {read: {operation, input: {operation: inputs.operation, account: inputs.account_id, ...(inputs.operation === 'navigate' ? {url: inputs.url} : {})}}};
}
export function complete(inputs) {
  ensure(inputs.record.status === 'committed', 'Provider readback exists but contribution history did not commit. Resume the original run.');
  return {result: {status: 'confirmed', account_id: inputs.contribution.account_id,
    action: inputs.contribution.action, permalink: inputs.contribution.permalink,
    visibility: inputs.contribution.visibility, readback_uri: inputs.provider_operation.readback_uri,
    contribution_id: inputs.record.event_id}};
}
