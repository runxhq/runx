#![allow(clippy::expect_used, clippy::unwrap_used)]

use runx_contracts::schema::NonEmptyString;

use super::*;

fn operation(cost: Option<serde_json::Value>) -> ConnectorOperation {
    let mut operation = serde_json::json!({
        "description": "Search.",
        "verb": "read",
        "scopes": ["web.search"],
        "target": {"template": "https://api.example.com"},
    });
    if let Some(cost) = cost {
        operation["cost"] = cost;
    }
    serde_json::from_value(operation).unwrap()
}

fn amount(units: u64, unit: &str) -> OperationAmount {
    OperationAmount {
        units,
        unit: NonEmptyString::from(unit),
    }
}

fn reserve(store: &KernelStore, run_id: &str, limit: u64, key: &str, usd_micro: u64) {
    let cover = cover(store, run_id, limit, key, "intent", usd_micro).unwrap();
    store
        .append(vec![cover.reservation().unwrap().unwrap()])
        .unwrap();
}

#[test]
fn a_call_spends_its_quote_and_the_dollars_it_moves() {
    let priced = operation(Some(
        serde_json::json!({"usd_micro": 5000, "per": "request", "source": "docs"}),
    ));
    assert_eq!(
        charge(&priced, None).unwrap(),
        Charge {
            usd_micro: 5000,
            on_success: false
        }
    );
    let on_success = operation(Some(
        serde_json::json!({"usd_micro": 26000, "per": "success", "source": "docs"}),
    ));
    assert!(charge(&on_success, None).unwrap().on_success);

    let unpriced = operation(None);
    assert_eq!(charge(&unpriced, None).unwrap().usd_micro, 0);
    // USDC's six decimals are micro-USD; USD is counted in cents.
    assert_eq!(
        charge(&unpriced, Some(&amount(1_500_000, "USDC")))
            .unwrap()
            .usd_micro,
        1_500_000
    );
    assert_eq!(
        charge(&unpriced, Some(&amount(150, "USD")))
            .unwrap()
            .usd_micro,
        1_500_000
    );
    // A payment is billable whether or not the provider answers well.
    assert!(
        !charge(&on_success, Some(&amount(1, "USDC")))
            .unwrap()
            .on_success
    );
    let refused = charge(&unpriced, Some(&amount(100, "EUR"))).unwrap_err();
    assert!(
        refused.contains("a payment in EUR cannot be counted"),
        "{refused}"
    );
}

#[test]
fn a_call_over_what_is_left_is_refused_and_a_release_frees_it() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    reserve(&store, "run-1", 10_000, "call-1", 6_000);
    let refused = cover(&store, "run-1", 10_000, "call-2", "intent", 5_000).unwrap_err();
    assert!(
        matches!(
            refused,
            SpendError::Exceeded {
                limit: 10_000,
                remaining: 4_000,
                usd_micro: 5_000
            }
        ),
        "{refused}"
    );
    assert!(refused.to_string().starts_with("cost_limit_exceeded"));

    store
        .append(vec![release(&store, "run-1", "call-1").unwrap().unwrap()])
        .unwrap();
    assert_eq!(status(&store, "run-1").unwrap().committed_usd_micro, 0);
    reserve(&store, "run-1", 10_000, "call-2", 5_000);
}

#[test]
fn a_settled_spend_stays_spent_and_closes_once() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    reserve(&store, "run-1", 10_000, "call-1", 6_000);
    store
        .append(vec![settle(&store, "run-1", "call-1").unwrap().unwrap()])
        .unwrap();
    assert_eq!(status(&store, "run-1").unwrap().committed_usd_micro, 6_000);
    assert!(settle(&store, "run-1", "call-1").unwrap().is_none());
    assert!(release(&store, "run-1", "call-1").unwrap().is_none());
    // An attempt that reserved nothing has nothing to close.
    assert!(settle(&store, "run-1", "never").unwrap().is_none());
}

#[test]
fn child_runs_spend_from_their_roots_limit() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    reserve(&store, "run-1/research", 10_000, "call-1", 7_000);
    assert!(matches!(
        cover(
            &store,
            "run-1/research/fetch",
            10_000,
            "call-2",
            "intent",
            4_000
        ),
        Err(SpendError::Exceeded {
            remaining: 3_000,
            ..
        })
    ));
    // Another run has its own limit.
    assert!(cover(&store, "run-2", 10_000, "call-3", "intent", 4_000).is_ok());
}

#[test]
fn a_recovered_attempt_is_covered_by_its_reservation() {
    let root = tempfile::tempdir().unwrap();
    let store = KernelStore::open(root.path()).unwrap();
    reserve(&store, "run-1", 10_000, "call-1", 10_000);
    let recovered = cover(&store, "run-1", 10_000, "call-1", "intent", 10_000).unwrap();
    assert!(recovered.reservation().unwrap().is_none());
}
