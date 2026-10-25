const asObject = (value) => value && typeof value === 'object' && !Array.isArray(value) ? value : {};
const asText = (value, max = 1024) => typeof value === 'string' && value.length <= max ? value : '';
const instant = (value) => /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?(?:Z|[+-]\d{2}:\d{2})$/.test(value) && !Number.isNaN(Date.parse(value));
const date = (value) => /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(Date.parse(`${value}T00:00:00Z`));

export function normalizeCalendar(input) {
  const result = asObject(input.provider_result);
  const kind = input.kind === 'freebusy' ? 'freebusy' : 'events';
  const sourceStatus = input.source_status === 'provider_readback' ? 'provider_readback' : 'supplied_result';
  const expectedCalendar = asText(input.calendar_id, 255);
  const calendarId = asText(result.calendar_id, 255);
  const timeMin = asText(result.time_min, 40);
  const timeMax = asText(result.time_max, 40);
  const problems = [];
  if (!calendarId || calendarId !== expectedCalendar) problems.push('calendar_target_mismatch');
  if (!instant(timeMin) || !instant(timeMax) || Date.parse(timeMax) <= Date.parse(timeMin) || Date.parse(timeMax) - Date.parse(timeMin) > 30 * 86400000) problems.push('window_invalid');
  if (input.time_min !== timeMin || input.time_max !== timeMax) problems.push('window_mismatch');
  if (!instant(asText(result.fetched_at, 40))) problems.push('observation_time_invalid');
  const complete = result.complete === true;
  const context = {
    schema: 'runx/google-calendar.context.v1',
    kind,
    provider: 'google-calendar',
    source_status: sourceStatus,
    provider_status: sourceStatus === 'provider_readback' ? 'readback_verified' : 'not_called',
    calendar_id: calendarId,
    time_min: timeMin,
    time_max: timeMax,
    observed_at: asText(result.fetched_at, 40),
    coverage: complete ? 'complete' : 'partial',
  };
  if (kind === 'events') {
    if (!Array.isArray(result.events) || result.events.length > 2500) problems.push('events_invalid');
    context.events = (Array.isArray(result.events) ? result.events.slice(0, 2500) : []).map((raw) => {
      const event = asObject(raw);
      const allDay = event.all_day === true;
      const start = asText(event.start, 100);
      const end = asText(event.end, 100);
      if (!asText(event.event_id) || (event.status !== 'cancelled' || start || end) && (!(allDay ? date(start) && date(end) : instant(start) && instant(end)) || Date.parse(allDay ? `${end}T00:00:00Z` : end) <= Date.parse(allDay ? `${start}T00:00:00Z` : start))) problems.push('event_time_invalid');
      return {
        event_id: asText(event.event_id), revision: asText(event.revision), status: asText(event.status, 40),
        visibility: asText(event.visibility, 40), transparency: asText(event.transparency, 40),
        ...(event.visibility === 'private' ? {} : { summary: asText(event.summary, 500) }),
        all_day: allDay, ...(start ? { start } : {}), ...(end ? { end } : {}),
        ...(asText(event.time_zone, 100) ? { time_zone: event.time_zone } : {}),
        ...(asText(event.recurring_event_id) ? { recurring_event_id: event.recurring_event_id } : {}),
        ...(event.original_start ? { original_start: asObject(event.original_start) } : {}),
        self_response: asText(event.self_response, 50),
      };
    });
    if (!complete) context.next_page_token = asText(result.next_page_token, 2000);
  } else {
    if (!Array.isArray(result.busy) || result.busy.length > 2500) problems.push('busy_invalid');
    context.errors = (Array.isArray(result.errors) ? result.errors.slice(0, 20) : []).map((error) => ({ reason: asText(asObject(error).reason, 200) }));
    context.busy = (Array.isArray(result.busy) ? result.busy.slice(0, 2500) : []).map((raw) => {
      const item = asObject(raw); const start = asText(item.start, 40); const end = asText(item.end, 40);
      if (!instant(start) || !instant(end) || Date.parse(end) <= Date.parse(start)) problems.push('busy_interval_invalid');
      return { start, end };
    });
    if (context.errors.length) context.coverage = 'partial';
  }
  context.decision = problems.length ? 'blocked' : context.coverage === 'complete' ? 'ready' : 'partial';
  context.validation = { status: problems.length ? 'fail' : 'pass', findings: [...new Set(problems)] };
  return { calendar_context: context };
}

export function prepareAssistantWindow(input) {
  const match = /^runx:\/\/assistant\/goal\/[a-z0-9_-]+\/schedule\/sha256:[0-9a-f]{64}\/occurrence\/([0-9]+)$/.exec(input.occurrence_uri);
  const dueSeconds = match ? Number(match[1]) : NaN;
  const hours = Number(input.window_hours);
  if (!Number.isSafeInteger(dueSeconds) || dueSeconds <= 0
      || !Number.isSafeInteger(hours) || hours < 1 || hours > 168) {
    throw new Error('calendar brief requires a bounded admitted occurrence and window');
  }
  if (!instant(input.observed_at)) {
    throw new Error('calendar brief requires an explicit host observation instant');
  }
  // The host owns scheduling and the observation time; the worker has no
  // ambient clock. Checkpoint this exact upcoming window in the graph.
  const start = Math.max(dueSeconds * 1000, Date.parse(input.observed_at));
  const end = start + hours * 3600000;
  if (!Number.isSafeInteger(end) || end > 8.64e15) {
    throw new Error('calendar brief window exceeds the supported clock range');
  }
  return { calendar_window: {
    calendar_id: input.calendar_id,
    occurrence_uri: input.occurrence_uri,
    time_min: new Date(start).toISOString(),
    time_max: new Date(end).toISOString(),
  } };
}

export function finishAssistantBrief(input) {
  const context = asObject(input.calendar_context);
  const window = asObject(input.window);
  const bound = context.schema === 'runx/google-calendar.context.v1'
    && context.kind === 'events'
    && context.provider === 'google-calendar'
    && context.calendar_id === input.calendar_id
    && context.time_min === window.time_min && context.time_max === window.time_max
    && window.occurrence_uri === input.occurrence_uri
    && context.source_status === 'provider_readback'
    && context.provider_status === 'readback_verified';
  if (!bound) throw new Error('calendar brief differs from its provider-backed window');
  if (context.decision !== 'ready' || context.coverage !== 'complete'
      || context.validation?.status !== 'pass') {
    return { work_result: {
      state: 'held', summary: 'Calendar coverage is incomplete; upcoming availability is unknown.',
      completion_evidence: [], followups: [],
    } };
  }
  const events = (Array.isArray(context.events) ? context.events : [])
    .filter((event) => event.status !== 'cancelled')
    .sort((left, right) => String(left.start || '').localeCompare(String(right.start || '')));
  const upcoming = events.slice(0, 3).map((event) => {
    const label = event.visibility === 'private' ? 'private event' : asText(event.summary, 100) || 'untitled event';
    return `${event.start || 'time unknown'} ${label}`;
  });
  const summary = `Calendar: ${events.length} active event${events.length === 1 ? '' : 's'} in the next window.`
    + (upcoming.length ? ` ${upcoming.join('; ')}` : '');
  return { work_result: {
    state: 'satisfied', summary: summary.slice(0, 1000),
    completion_evidence: [
      `google-calendar:${input.calendar_id}@${context.observed_at}:${window.time_min}..${window.time_max}`,
    ],
    followups: [],
  } };
}
