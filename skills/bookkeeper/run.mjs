import fs from "node:fs";

const raw = process.env.RUNX_INPUTS_PATH
  ? fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8")
  : process.env.RUNX_INPUTS_JSON || "{}";
const inputs = JSON.parse(raw);
const result = reconcile(inputs);
process.stdout.write(`${JSON.stringify(result)}\n`);

function reconcile(inputs) {
  const transactions = requiredArray(inputs.transactions, "transactions");
  const chart = requiredArray(inputs.chart_of_accounts, "chart_of_accounts");
  const priorPeriod = requiredObject(inputs.prior_period, "prior_period");
  if (transactions.length === 0) throw new Error("transactions must not be empty");
  if (chart.length === 0) throw new Error("chart_of_accounts must not be empty");

  const startDate = isoDate(priorPeriod.start_date, "prior_period.start_date");
  const endDate = isoDate(priorPeriod.end_date, "prior_period.end_date");
  if (startDate > endDate) throw new Error("prior_period.start_date must not follow end_date");

  const accounts = new Map();
  for (const [index, value] of chart.entries()) {
    const account = requiredObject(value, `chart_of_accounts[${index}]`);
    const code = nonempty(account.code, `chart_of_accounts[${index}].code`);
    if (accounts.has(code)) throw new Error(`duplicate chart account code: ${code}`);
    const name = nonempty(account.name, `chart_of_accounts[${index}].name`);
    const keywords = account.keywords === undefined
      ? []
      : requiredArray(account.keywords, `chart_of_accounts[${index}].keywords`)
          .map((keyword, keywordIndex) => nonempty(
            keyword,
            `chart_of_accounts[${index}].keywords[${keywordIndex}]`,
          ));
    const terms = new Set([...tokens(name), ...keywords.flatMap(tokens)]);
    accounts.set(code, { code, name, type: text(account.type), terms });
  }

  const seenIds = new Set();
  const categorized = [];
  const anomalies = [];
  const unmatched = [];

  for (const [index, value] of transactions.entries()) {
    const transaction = requiredObject(value, `transactions[${index}]`);
    const id = nonempty(transaction.id, `transactions[${index}].id`);
    if (seenIds.has(id)) throw new Error(`duplicate transaction id: ${id}`);
    seenIds.add(id);
    const date = isoDate(transaction.date, `transactions[${index}].date`);
    const description = nonempty(transaction.description, `transactions[${index}].description`);
    const amount = finiteNonzero(transaction.amount, `transactions[${index}].amount`);
    const currency = transaction.currency === undefined
      ? null
      : nonempty(transaction.currency, `transactions[${index}].currency`).toUpperCase();

    if (date < startDate || date > endDate) {
      anomalies.push({
        transaction_id: id,
        code: "out_of_period",
        reason: `transaction date ${date} is outside ${startDate} through ${endDate}`,
      });
    }

    const explicitCode = transaction.account_code === undefined
      ? null
      : nonempty(transaction.account_code, `transactions[${index}].account_code`);
    if (explicitCode !== null) {
      const account = accounts.get(explicitCode);
      if (!account) {
        unmatched.push(unmatchedLine(id, date, description, amount, currency, "unknown_explicit_account"));
        anomalies.push({
          transaction_id: id,
          code: "unknown_explicit_account",
          reason: `account_code ${explicitCode} is not present in chart_of_accounts`,
        });
        continue;
      }
      categorized.push(categorizedLine({
        id,
        date,
        description,
        amount,
        currency,
        account,
        confidence: 1,
        reason: "exact account_code supplied and found in chart_of_accounts",
        matchedTerms: [],
      }));
      continue;
    }

    const descriptionTerms = new Set(tokens(description));
    const ranked = [...accounts.values()]
      .map((account) => ({
        account,
        matchedTerms: [...descriptionTerms].filter((term) => account.terms.has(term)).sort(),
      }))
      .filter((candidate) => candidate.matchedTerms.length > 0)
      .sort((left, right) => right.matchedTerms.length - left.matchedTerms.length
        || left.account.code.localeCompare(right.account.code));

    if (ranked.length === 0) {
      unmatched.push(unmatchedLine(id, date, description, amount, currency, "no_account_match"));
      anomalies.push({
        transaction_id: id,
        code: "no_account_match",
        reason: "no chart account name or keyword matched the transaction description",
      });
      continue;
    }
    if (ranked.length > 1 && ranked[0].matchedTerms.length === ranked[1].matchedTerms.length) {
      unmatched.push(unmatchedLine(id, date, description, amount, currency, "ambiguous_account_match"));
      anomalies.push({
        transaction_id: id,
        code: "ambiguous_account_match",
        reason: `multiple chart accounts tie at ${ranked[0].matchedTerms.length} matched term(s)`,
        candidate_account_codes: ranked
          .filter((candidate) => candidate.matchedTerms.length === ranked[0].matchedTerms.length)
          .map((candidate) => candidate.account.code),
      });
      continue;
    }

    const winner = ranked[0];
    const confidence = Math.min(0.99, Math.round((0.7 + winner.matchedTerms.length * 0.1) * 100) / 100);
    categorized.push(categorizedLine({
      id,
      date,
      description,
      amount,
      currency,
      account: winner.account,
      confidence,
      reason: `unique chart match on: ${winner.matchedTerms.join(", ")}`,
      matchedTerms: winner.matchedTerms,
    }));
  }

  const reasonCodes = [...new Set([
    ...unmatched.map((line) => line.reason_code),
    ...anomalies.map((anomaly) => anomaly.code),
  ])].sort();
  const reviewRequired = reasonCodes.length > 0;
  const reconciliation = {
    status: reviewRequired ? "needs_review" : "reconciled",
    matched: categorized.length,
    unmatched: unmatched.length,
    transaction_count: transactions.length,
    matched_amount: money(categorized.reduce((total, line) => total + line.transaction.amount, 0)),
    unmatched_amount: money(unmatched.reduce((total, line) => total + line.amount, 0)),
    unmatched_transactions: unmatched,
    period: { start_date: startDate, end_date: endDate },
  };

  return {
    categorized,
    anomalies,
    reconciliation,
    needs_review: {
      required: reviewRequired,
      reasons: reasonCodes,
      reason: !reviewRequired
        ? null
        : `${unmatched.length} unmatched transaction(s) and ${anomalies.length} anomaly finding(s) require review`,
    },
    read_only: {
      ledger_mutation: false,
      write_count: 0,
      writes: [],
      external_effects: [],
      statement: "reconciliation artifact only; no journal entry was posted",
    },
  };
}

function categorizedLine({ id, date, description, amount, currency, account, confidence, reason, matchedTerms }) {
  return {
    transaction: { id, date, description, amount, currency },
    account: { code: account.code, name: account.name, type: account.type },
    confidence,
    reason,
    matched_terms: matchedTerms,
  };
}

function unmatchedLine(id, date, description, amount, currency, reasonCode) {
  return { id, date, description, amount, currency, reason_code: reasonCode };
}

function tokens(value) {
  return String(value)
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .split(/\s+/)
    .filter((token) => token.length >= 2);
}

function isoDate(value, name) {
  const candidate = nonempty(value, name);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(candidate)) throw new Error(`${name} must be an ISO date`);
  const date = new Date(`${candidate}T00:00:00Z`);
  if (Number.isNaN(date.valueOf()) || date.toISOString().slice(0, 10) !== candidate) {
    throw new Error(`${name} must be a real calendar date`);
  }
  return candidate;
}

function finiteNonzero(value, name) {
  if (typeof value !== "number" || !Number.isFinite(value) || value === 0) {
    throw new Error(`${name} must be a finite non-zero number`);
  }
  return value;
}

function requiredArray(value, name) {
  if (!Array.isArray(value)) throw new Error(`${name} must be an array`);
  return value;
}

function requiredObject(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value;
}

function nonempty(value, name) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} must be a non-empty string`);
  }
  return value.trim();
}

function text(value) {
  return typeof value === "string" ? value : null;
}

function money(value) {
  return Math.round((value + Number.EPSILON) * 100) / 100;
}
