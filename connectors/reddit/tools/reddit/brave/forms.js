// Fixed Reddit DOM operations. The caller supplies data, never JavaScript.
function redditForm(input) {
  const all = selector => Array.from(document.querySelectorAll(selector));
  const visible = element => element && !element.hidden && element.getClientRects().length > 0;
  const normalize = value => value.replace(/\r\n?/g, '\n');
  const fullname = element => element?.getAttribute('thingid') || element?.getAttribute('data-fullname') || '';
  const bodyOf = element => {
    const body = element.querySelector('[slot="comment"], .usertext-body .md, [slot="text-body"]');
    return body ? normalize(body.innerText) : '';
  };
  const commentElements = () => all('shreddit-comment, .thing.comment');
  const authorOf = element => element.getAttribute('author') || element.querySelector('a.author')?.textContent || '';
  const thread = () => document.querySelector('shreddit-post, .thing.link');
  const identity = () => {
    const link = document.querySelector('.user a[href*="/user/"]');
    return link ? new URL(link.href).pathname.split('/')[2] : null;
  };
  const error = reason => ({error: reason});
  const records = () => commentElements().map(element => {
    const id = fullname(element);
    const link = element.querySelector('a.bylink, a[slot="commentPermalink"], a[permalink]');
    const explicit = element.getAttribute('permalink');
    const parent = element.parentElement?.closest('shreddit-comment, .thing.comment');
    return {
      id, author: authorOf(element), body: bodyOf(element),
      parent_id: element.getAttribute('parentid') || element.getAttribute('data-parent-fullname')
        || fullname(parent) || (element.getAttribute('depth') === '0'
          || (element.matches('.thing.comment') && !parent) ? input.thread_id : ''),
      permalink: explicit ? new URL(explicit, location.origin).href : link?.href || '',
    };
  });
  if (input.command === 'identity') return {username: identity()};
  if (input.command === 'comments') return {comments: records()};
  if (input.command === 'readback') {
    if (input.kind === 'comment') {
      const record = records().find(item => item.id === input.content_id);
      return record || error('comment_readback_missing');
    }
    const element = thread();
    if (!element) return error('post_readback_missing');
    return {
      id: fullname(element) || element.id,
      author: authorOf(element), body: bodyOf(element),
      title: element.getAttribute('post-title') || element.querySelector('a.title')?.textContent || '',
      permalink: location.href,
    };
  }
  if (input.kind === 'upvote') {
    if (identity()?.toLowerCase() !== input.account) return error('wrong_visible_account');
    if (location.href.replace(/\/$/, '') !== input.target_url.replace(/\/$/, '')) return error('publication_target_changed');
    const element = input.parent_id?.startsWith('t1_')
      ? commentElements().find(element => fullname(element) === input.parent_id) : thread();
    if (!element || fullname(element) !== input.parent_id) return error('vote_target_missing');
    if (authorOf(element).toLowerCase() !== input.target_author) return error('vote_target_author_mismatch');
    const body = bodyOf(element) || element.querySelector('a.title')?.textContent || '';
    if (body !== input.body) return error('vote_target_body_mismatch');
    const controls = element.querySelector(':scope > .midcol');
    const button = controls?.querySelector('.arrow.up, .arrow.upmod');
    if (!visible(button)) return error('background_vote_control_unavailable');
    const selected = button.classList.contains('upmod');
    if (input.command === 'vote') {
      if (!selected) button.click();
      return {submitted: true};
    }
    return {vote_state: selected ? 'up' : 'none', author: authorOf(element), body};
  }
  const parent = input.parent_id?.startsWith('t1_')
    ? commentElements().find(element => fullname(element) === input.parent_id) : null;
  if (input.parent_id?.startsWith('t1_') && !parent) return error('reply_parent_not_rendered');
  const scope = parent || document;
  if (input.command === 'prepare' && parent && !visible(parent.querySelector('form.usertext textarea'))) {
    const reply = parent.querySelector(':scope > .entry a[onclick*="reply"]');
    if (!visible(reply)) return error('reply_control_unavailable');
    reply.click();
  }
  const form = input.kind === 'post' ? document.querySelector('form#newlink')
    : Array.from((parent || document.querySelector('.commentarea') || document).querySelectorAll('form.usertext, form#commentform'))
      .find(candidate => visible(candidate.querySelector('textarea[name="text"], textarea[name="body"]'))
        && candidate.querySelector('input[name="thing_id"], input[name="parent"]')?.value === input.parent_id);
  if (input.kind === 'post' && input.command === 'prepare' && !visible(form?.querySelector('textarea[name="text"]'))) {
    const tab = form?.querySelector('a.self-button');
    if (visible(tab)) tab.click();
  }
  const editor = form?.querySelector('textarea[name="text"], textarea[name="body"]');
  const title = form?.querySelector('textarea[name="title"], input[name="title"]');
  if (!form || !visible(editor)) return error('background_plain_text_editor_unavailable');
  if (input.kind === 'post') {
    const community = form.querySelector('input[name="sr"]')?.value;
    const kinds = Array.from(form.querySelectorAll('input[name="kind"]')).filter(element => !element.disabled);
    if (community?.toLowerCase() !== input.subreddit.toLowerCase()) return error('composer_community_mismatch');
    if (kinds.length !== 1 || kinds[0].value !== 'self') return error('composer_is_not_text_post');
  }
  const declaredParent = form.querySelector('input[name="thing_id"], input[name="parent"]')?.value;
  if (input.kind === 'comment' && declaredParent !== input.parent_id) return error('composer_parent_mismatch');
  const button = form.querySelector('button[type="submit"], input[type="submit"]');
  if (!visible(button) || button.disabled) return error('submission_unavailable');
  if (input.command === 'prepare') {
    if (editor.value && normalize(editor.value) !== input.body) return error('existing_draft_present');
    if (input.kind === 'post') {
      if (!visible(title)) return error('post_title_editor_unavailable');
      if (title.value && title.value !== input.title) return error('existing_title_present');
      title.value = input.title;
      title.dispatchEvent(new Event('input', {bubbles: true}));
    }
    editor.value = input.body;
    editor.dispatchEvent(new Event('input', {bubbles: true}));
    editor.dispatchEvent(new Event('change', {bubbles: true}));
    return {body: normalize(editor.value), title: title?.value || ''};
  }
  if (input.command === 'submit') {
    if (identity()?.toLowerCase() !== input.account) return error('wrong_visible_account');
    if (location.href.replace(/\/$/, '') !== input.target_url.replace(/\/$/, '')) return error('publication_target_changed');
    if (normalize(editor.value) !== input.body || (input.kind === 'post' && title?.value !== input.title)) {
      return error('editor_content_mismatch');
    }
    const errors = Array.from(form.querySelectorAll('.error')).filter(visible).map(e => e.textContent);
    if (errors.some(text => text.trim())) return error('form_validation_error');
    button.click();
    return {submitted: true};
  }
  return error('unsupported_form_command');
}
