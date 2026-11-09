//! A script pass on its own: what it reports, and replay in settlement order
//! (plan §9's control-flow row).

use std::collections::BTreeMap;

use serde_json::json;

use super::{Passed, ScriptPass, pass, start};
use crate::protocol::{
    InvocationLimits, ScriptCallOutcome, ScriptCallTarget, ScriptEnding, ScriptRejection,
    ScriptSettlement,
};

const NOW: i64 = 1_800_000_000_000;

fn run(source: &str, settled: &[ScriptSettlement]) -> Result<Passed, String> {
    let modules = BTreeMap::from([("main.mjs".to_owned(), source.to_owned())]);
    pass(ScriptPass {
        entry_module: "main.mjs",
        export_name: "default",
        modules: &modules,
        inputs: json!({"lead": "ada"}),
        limits: InvocationLimits::default(),
        settled,
        now: NOW,
    })
    .map_err(|error| error.message)
}

fn resolved(position: u32, value: serde_json::Value) -> ScriptSettlement {
    ScriptSettlement {
        position,
        outcome: ScriptCallOutcome::Resolved {
            value,
            references: Vec::new(),
        },
    }
}

fn targets(passed: &Passed) -> Vec<(u32, String)> {
    passed
        .calls
        .iter()
        .map(|call| {
            let named = match &call.target {
                ScriptCallTarget::Skill { reference, .. } => format!("skill {reference}"),
                ScriptCallTarget::Tool { name, .. } => format!("tool {name}"),
                ScriptCallTarget::Operation { operation, .. } => format!("operation {operation}"),
                ScriptCallTarget::Now { millis } => format!("now {millis}"),
                ScriptCallTarget::Wait { until } => format!("wait {until}"),
                ScriptCallTarget::Read { digest } => format!("read {digest}"),
            };
            (call.position, named)
        })
        .collect()
}

#[test]
fn a_pass_reports_each_call_at_its_position_and_waits_on_them() -> Result<(), String> {
    let passed = run(
        r#"export default async function (ctx, { lead }) {
            const [checked, drafted] = await Promise.all([
                ctx.tool("validate", { lead }),
                ctx.skill(".#draft", { lead }),
            ]);
            return { checked, drafted };
        }"#,
        &[],
    )?;
    assert_eq!(passed.ending, ScriptEnding::Waiting);
    assert_eq!(
        targets(&passed),
        [
            (0, "tool validate".to_owned()),
            (1, "skill .#draft".to_owned())
        ]
    );
    assert_eq!(
        passed.calls[1].target,
        ScriptCallTarget::Skill {
            reference: ".#draft".to_owned(),
            inputs: json!({"lead": "ada"}),
        }
    );
    Ok(())
}

#[test]
fn a_script_returns_once_its_record_settles_every_call() -> Result<(), String> {
    let source = r#"export default async function (ctx, { lead }) {
        const checked = await ctx.tool("validate", { lead });
        const drafted = await ctx.skill(".#draft", { lead, checked });
        return { drafted };
    }"#;
    let first = run(source, &[resolved(0, json!({"passed": true}))])?;
    assert_eq!(first.ending, ScriptEnding::Waiting);
    assert_eq!(
        first.calls[1].target,
        ScriptCallTarget::Skill {
            reference: ".#draft".to_owned(),
            inputs: json!({"lead": "ada", "checked": {"passed": true}}),
        }
    );
    let done = run(
        source,
        &[
            resolved(0, json!({"passed": true})),
            resolved(1, json!({"text": "hello"})),
        ],
    )?;
    assert_eq!(
        done.ending,
        ScriptEnding::Returned {
            output: json!({"drafted": {"text": "hello"}}),
        }
    );
    Ok(())
}

#[test]
fn settlements_replay_in_the_order_they_settled() -> Result<(), String> {
    // A waits on approval and B settles first, so afterB's call takes the
    // next position. Delivered in recorded order, replay gives it the same.
    let source = r#"export default async function (ctx) {
        const a = ctx.tool("a", {}).then(() => ctx.tool("after_a", {}));
        const b = ctx.tool("b", {}).then(() => ctx.tool("after_b", {}));
        await Promise.all([a, b]);
        return {};
    }"#;
    let b_first = run(source, &[resolved(1, json!({})), resolved(0, json!({}))])?;
    assert_eq!(
        targets(&b_first),
        [
            (0, "tool a".to_owned()),
            (1, "tool b".to_owned()),
            (2, "tool after_b".to_owned()),
            (3, "tool after_a".to_owned()),
        ]
    );
    let a_first = run(source, &[resolved(0, json!({})), resolved(1, json!({}))])?;
    assert_eq!(a_first.calls[2].target, b_first.calls[3].target);
    assert_eq!(a_first.calls[3].target, b_first.calls[2].target);
    Ok(())
}

#[test]
fn a_call_that_did_not_succeed_rejects_with_its_reason_code_and_message() -> Result<(), String> {
    let passed = run(
        r#"export default async function (ctx) {
            try {
                await ctx.operation("github/issues.update", { repository: "a/b" }, { idempotency_key: "k" });
                return { caught: null };
            } catch (rejected) {
                return { caught: { reason: rejected.reason, code: rejected.code, message: rejected.message } };
            }
        }"#,
        &[ScriptSettlement {
            position: 0,
            outcome: ScriptCallOutcome::Rejected {
                reason: ScriptRejection::Refused,
                code: "outside_calls".to_owned(),
                message: "github/issues.update is not in the step's calls".to_owned(),
            },
        }],
    )?;
    assert_eq!(
        passed.ending,
        ScriptEnding::Returned {
            output: json!({"caught": {
                "reason": "refused",
                "code": "outside_calls",
                "message": "github/issues.update is not in the step's calls",
            }}),
        }
    );
    assert_eq!(
        passed.calls[0].target,
        ScriptCallTarget::Operation {
            operation: "github/issues.update".to_owned(),
            inputs: json!({"repository": "a/b"}),
            idempotency_key: Some("k".to_owned()),
        }
    );
    Ok(())
}

#[test]
fn a_record_naming_a_call_the_script_did_not_make_diverges() -> Result<(), String> {
    let passed = run(
        r#"export default async function (ctx) { return { value: await ctx.tool("a", {}) }; }"#,
        &[resolved(0, json!({})), resolved(4, json!({}))],
    )?;
    assert_eq!(passed.ending, ScriptEnding::Diverged { position: 4 });
    Ok(())
}

#[test]
fn time_comes_from_ctx_now_alone() -> Result<(), String> {
    let source = r#"export default async function (ctx) {
        const started = ctx.now();
        await ctx.tool("a", {});
        const refused = [];
        for (const read of [() => Date.now(), () => new Date(), () => Date(), () => Math.random()]) {
            try { read(); refused.push(false); } catch { refused.push(true); }
        }
        return { started, ended: ctx.now(), year: new Date(started).getUTCFullYear(), refused };
    }"#;
    let first = run(source, &[])?;
    assert_eq!(
        targets(&first),
        [(0, format!("now {NOW}")), (1, "tool a".to_owned())]
    );
    // Replay reads what the record holds, not the pass's time.
    let replayed = run(
        source,
        &[
            ScriptSettlement {
                position: 0,
                outcome: ScriptCallOutcome::Now { millis: 1_000 },
            },
            resolved(1, json!({})),
        ],
    )?;
    let ScriptEnding::Returned { output } = replayed.ending else {
        return Err(format!("the script returns: {:?}", replayed.ending));
    };
    assert_eq!(output["started"].as_f64(), Some(1_000.0));
    // A ctx.now() past the record reads the pass's time.
    #[allow(clippy::cast_precision_loss)]
    let now = NOW as f64;
    assert_eq!(output["ended"].as_f64(), Some(now));
    assert_eq!(output["year"].as_f64(), Some(1_970.0));
    assert_eq!(output["refused"], json!([true, true, true, true]));
    Ok(())
}

#[test]
fn a_return_with_a_call_outstanding_is_reported_with_that_call() -> Result<(), String> {
    let passed = run(
        r#"export default async function (ctx) { ctx.tool("left_running", {}); return { done: true }; }"#,
        &[],
    )?;
    assert_eq!(
        passed.ending,
        ScriptEnding::Returned {
            output: json!({"done": true}),
        }
    );
    assert_eq!(targets(&passed), [(0, "tool left_running".to_owned())]);
    Ok(())
}

#[test]
fn a_throw_ends_the_pass_with_its_message() -> Result<(), String> {
    let passed = run(
        r#"export default async function (ctx) { await ctx.tool("a", {}); throw new Error("no draft passed"); }"#,
        &[resolved(0, json!({}))],
    )?;
    assert_eq!(
        passed.ending,
        ScriptEnding::Threw {
            message: "no draft passed".to_owned(),
        }
    );
    Ok(())
}

#[test]
fn a_live_script_takes_its_settlements_as_a_replay_of_them_would() -> Result<(), String> {
    // B settles before A, then each continuation's call settles.
    let source = r#"export default async function (ctx) {
        const a = ctx.tool("a", {}).then((value) => ctx.tool("after_a", value));
        const b = ctx.tool("b", {}).then((value) => ctx.tool("after_b", value));
        return { results: await Promise.all([a, b]) };
    }"#;
    let order = [
        resolved(1, json!({"from": "b"})),
        resolved(0, json!({"from": "a"})),
        resolved(2, json!({"after": "b"})),
        resolved(3, json!({"after": "a"})),
    ];
    let modules = BTreeMap::from([("main.mjs".to_owned(), source.to_owned())]);
    let (live, first) = start(ScriptPass {
        entry_module: "main.mjs",
        export_name: "default",
        modules: &modules,
        inputs: json!({}),
        limits: InvocationLimits::default(),
        settled: &[],
        now: NOW,
    })
    .map_err(|error| error.message)?;
    let mut live = live.ok_or("a waiting script stays live")?;
    let mut calls = first.calls;
    let mut ending = first.ending;
    for settlement in &order {
        let passed = live
            .settle(std::slice::from_ref(settlement), NOW)
            .map_err(|error| error.message)?;
        calls.extend(passed.calls);
        ending = passed.ending;
    }
    let replayed = run(source, &order)?;
    assert_eq!(calls, replayed.calls);
    assert_eq!(ending, replayed.ending);
    assert_eq!(
        ending,
        ScriptEnding::Returned {
            output: json!({"results": [{"after": "a"}, {"after": "b"}]}),
        }
    );
    // Live, each report holds only the calls made since the last.
    assert_eq!(
        targets(&Passed {
            calls: calls[2..].to_vec(),
            ending: ScriptEnding::Waiting,
        }),
        [
            (2, "tool after_b".to_owned()),
            (3, "tool after_a".to_owned())
        ]
    );
    Ok(())
}

#[test]
fn each_settlement_drains_under_a_job_budget_of_its_own() -> Result<(), String> {
    // Far more awaits than one drain's budget allows, across settlements.
    let source = r#"export default async function (ctx) {
        let total = 0;
        for (let call = 0; call < 5000; call++) total += (await ctx.tool("count", {})).one;
        return { total };
    }"#;
    let settled = (0..5000)
        .map(|position| resolved(position, json!({"one": 1})))
        .collect::<Vec<_>>();
    let replayed = run(source, &settled)?;
    let ScriptEnding::Returned { output } = replayed.ending else {
        return Err(format!("the script returns: {:?}", replayed.ending));
    };
    assert_eq!(output["total"].as_f64(), Some(5_000.0));
    Ok(())
}
