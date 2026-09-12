---
name: bookkeeper
description: Categorize a bounded transaction batch against an existing chart of accounts and return a sealed, read-only reconciliation without inventing accounts or mutating a ledger. Use when an operator needs explainable GL mapping, anomaly flags, and an explicit needs-review lane.
registry_owner: ArgonautWorks
---

# Bookkeeper

Use this skill to reconcile a supplied transaction batch against a supplied
chart of accounts. The result is evidence for review, not authority to post a
journal entry. This skill performs no ledger mutation, bank action, payment,
file write, network request, or downstream dispatch.

## Operating model

1. Validate every transaction, account, and prior-period boundary before
   categorizing anything. Transaction identifiers and account codes must be
   unique. Dates must be real ISO calendar dates and amounts must be finite,
   non-zero numbers.
2. Treat `chart_of_accounts` as the entire account universe. Never create,
   infer, rename, or substitute a GL account outside that input.
3. Honor a transaction's explicit `account_code` only when that exact code is
   present in the chart. Otherwise compare normalized description tokens with
   each account's name and bounded `keywords` list.
4. Categorize only when one account has a unique positive match. Record the
   chart account code and name, confidence, reason, source transaction, and
   matched terms.
5. Leave tied, unknown, or explicitly invalid account bindings unmatched.
   Return `needs_review` with a concrete reason instead of guessing.
6. Flag duplicates and dates outside `prior_period.start_date` through
   `prior_period.end_date`. These anomalies remain read-only observations.
7. Reconcile the batch by reporting matched and unmatched counts and amount
   totals. Confirm the result's no-write proof before handing it to an
   operator.

## Inputs

- `transactions[]`: objects with `id`, ISO `date`, `description`, finite
  non-zero `amount`, and optional `currency` and `account_code`.
- `chart_of_accounts[]`: objects with unique `code`, `name`, optional `type`,
  and optional `keywords[]`. All keywords are evidence supplied by the caller.
- `prior_period`: an object with inclusive ISO `start_date` and `end_date`,
  plus optional prior reconciliation facts for operator context.

Do not paste secrets, bank credentials, card data, or unredacted personal data
into these inputs. Use stable transaction identifiers and already-admitted
accounting evidence.

## Outputs

- `categorized[]`: only matched lines. Every item binds to one exact input
  account and includes `confidence` and `reason`.
- `anomalies[]`: duplicate, out-of-period, or unmatched observations tied to a
  transaction identifier.
- `reconciliation`: `matched`, `unmatched`, counts, amount totals, and the
  overall `reconciled` or `needs_review` status.
- `needs_review`: whether human review is required and why.
- `read_only`: an explicit proof surface with `ledger_mutation: false`, empty
  `writes`, empty `external_effects`, and zero write count.

The receipt proves the bounded computation and output contract. It does not
prove a journal was posted because no posting occurs.

## Recovery rules

- Add an account to the chart only through the caller's normal chart-governance
  process, then rerun with the updated chart. This skill never adds it.
- Resolve a tie by supplying an exact existing `account_code` on the source
  transaction or by repairing the chart keywords. Do not lower the unique
  match requirement.
- Correct invalid dates, amounts, duplicate identifiers, or period bounds at
  the evidence source and rerun. Do not silently coerce them.
- A `needs_review` result is final for the supplied batch. No ledger write may
  be inferred from it.

## Agent rules

- Never invent a GL account, transaction, amount, date, confidence, or reason.
- Never mutate a ledger or claim that a read-only result was booked.
- Never hide unmatched transactions to make reconciliation appear complete.
- Keep every categorized line traceable to its transaction and chart account.
- Return the sealed receipt with the result for independent verification.
