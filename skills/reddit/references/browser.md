# Background browser contract

The connector owns browser mechanics. The skill owns contribution decisions and
voice. Native runx owns account instance pins, approval, custody and receipts.
No caller JavaScript, exported cookies, raw authenticated requests, global input,
clipboard, tab activation or foreground fallback is part of this interface.

## Set up an account

Use an existing normal Brave session. The browser must already allow JavaScript
from Apple Events; never change that permission or debugging flags automatically.
List Reddit tabs with the connector's fixed setup helper, select the intended
profile from operator context and bind its process/window/tab IDs to the expected username
and non-secret profile label. The helper verifies rendered identity before saving
an owner-only attachment cache. This is setup, not a proof that numeric IDs remain
valid after a restart. A missing tab requires a new verified binding.

Run the fixed setup helper from the connector package directory (for this source
checkout, `connectors/reddit`):

```bash
python3 -B tools/reddit/brave/brave.py tabs
python3 -B tools/reddit/brave/brave.py bind example_builder \
  --browser-pid 12345 --window-id 111 --tab-id 222 --profile-ref reddit-builder
runx connector use reddit --session \
  --parameter account=example_builder --parameter profile_ref=reddit-builder
```

Use the actual IDs returned by `tabs`, not those illustrative numbers. These two
helper commands attach an existing authenticated session; they do not publish,
vote, or replace the skill's normal typed browser operations. Run the same setup
again to rebind a missing tab, then confirm the native session. The native session
probe invokes the installed connector tool and pins the observed account. Every
call checks it again. A changed login, missing binding or different profile refuses
before an operation. Each tool invocation holds the same short local browser lock.
It stores attachment hints, not a second provider-effect ledger.

## Read

`read` takes `account_id`, a fixed `operation` and `url` only for navigation.
Operations are inspect, navigate, feed, rules, thread and composer. It returns the
native provider operation and its observation. Feed/rules/thread evidence comes
from bounded rendered modern-page observations; lazy loading is not full coverage.
If the evidence is missing, return the exact blocker or use public research.

At connector level, account.inspect, page.navigate, feed.read, rules.read,
thread.read and composer.read are reads.
Each carries a fixed operation selector. No generic evaluate or click API exists.

## Write

comment.create, post.create and vote.upvote use the native process protocol.
The kernel injects the pinned session account, perform/readback phase and native
provider key. The browser tool cannot accept an arbitrary script or submit through
its ordinary command-line read interface. The operation-owned native gate approves
the exact account, profile, target and content.

The included adapter supports explicitly selected old.reddit.com plain-text forms.
It matches the real parent, checks title/body and destination, refuses an unrelated
existing draft, pauses briefly, rechecks identity and performs one submit. A modern
rich editor is an explicit unsupported-editor refusal. Do not silently convert the
approved URL to another host or invent synthetic editor APIs.

An acknowledgement identifies the observed resulting permalink. A separate native
readback navigates independently and checks content ID, author, community, parent
and exact content. Visibility is visible_to_account; public visibility and moderation
acceptance are unknown. A timeout after a possible click is unknown, never a safe
refusal. Native custody must retain it. Upvotes set the up state and never toggle a
selected vote off on recovery.

## Stops

Wrong account/profile, foreground tab, browser busy, missing editor, validation
errors, challenge and navigation mismatch stop the operation. Do not retry through
another account or transport. Resume the same run after resolving a pre-effect
blocker. For an uncertain publication, use read-only investigation and native
custody; do not submit again or erase the evidence.

The earlier modern-editor probe failed to retain text in an inactive Brave tab.
That finding does not establish classic editor support. Local adapter and harness
checks are offline proof; live editor entry, submission and readback remain separate
provider proof, and must be reported as incomplete until actually observed.
