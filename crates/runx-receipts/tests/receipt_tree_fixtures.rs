use std::collections::BTreeMap;

use runx_contracts::{Receipt, Reference, ReferenceType};
use runx_receipts::{
    ReceiptResolveResult, ReceiptResolver, ReceiptTreeConfig, ReceiptVerification, ResolvedReceipt,
    verify_receipt_tree_with_resolver,
};
use serde::Deserialize;

const ORACLE: &str = include_str!("../../../fixtures/runtime/receipt-tree/oracle.json");

#[derive(Debug, Deserialize)]
struct Oracle {
    cases: Vec<TreeCase>,
    receipts: BTreeMap<String, Receipt>,
}

#[derive(Debug, Deserialize)]
struct TreeCase {
    name: String,
    root_receipt: String,
    supplied_child_receipts: Vec<String>,
    #[serde(default)]
    resolver_error_receipt_ids: Vec<String>,
    config: FixtureTreeConfig,
    expected: ExpectedVerification,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct FixtureTreeConfig {
    max_depth: usize,
    max_receipts: usize,
}

impl FixtureTreeConfig {
    fn to_receipt_config(self) -> ReceiptTreeConfig {
        ReceiptTreeConfig {
            max_depth: self.max_depth,
            max_receipts: self.max_receipts,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExpectedVerification {
    valid: bool,
    findings: Vec<ExpectedFinding>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedFinding {
    code: String,
    path: String,
}

#[test]
fn receipt_tree_fixture_oracle_matches_ordered_findings() -> Result<(), String> {
    let oracle: Oracle = serde_json::from_str(ORACLE).map_err(|error| error.to_string())?;

    for case in &oracle.cases {
        let root = oracle.receipt(&case.root_receipt)?;
        let children = oracle.child_receipts(&case.supplied_child_receipts)?;
        let resolver = FixtureResolver {
            children: &children,
            resolver_error_receipt_ids: &case.resolver_error_receipt_ids,
        };

        let verification =
            verify_receipt_tree_with_resolver(root, &resolver, case.config.to_receipt_config());

        assert_eq!(
            verification.valid, case.expected.valid,
            "validity drifted for fixture case {}",
            case.name
        );
        assert_eq!(
            ordered_findings(&verification),
            case.expected.findings,
            "ordered findings drifted for fixture case {}",
            case.name
        );
    }

    Ok(())
}

impl Oracle {
    fn receipt(&self, name: &str) -> Result<&Receipt, String> {
        self.receipts
            .get(name)
            .ok_or_else(|| format!("receipt fixture {name} is missing"))
    }

    fn child_receipts(&self, names: &[String]) -> Result<Vec<Receipt>, String> {
        names
            .iter()
            .map(|name| self.receipt(name).cloned())
            .collect()
    }
}

struct FixtureResolver<'a> {
    children: &'a [Receipt],
    resolver_error_receipt_ids: &'a [String],
}

impl ReceiptResolver for FixtureResolver<'_> {
    fn resolve_child<'a>(&'a self, reference: &Reference) -> ReceiptResolveResult<'a> {
        let Some(receipt_id) = referenced_receipt_id(reference) else {
            return ReceiptResolveResult::Malformed;
        };
        if self
            .resolver_error_receipt_ids
            .iter()
            .any(|id| id == receipt_id)
        {
            return ReceiptResolveResult::ResolverError;
        }
        let mut matches = self
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.id == receipt_id);
        let Some((index, receipt)) = matches.next() else {
            return ReceiptResolveResult::Missing;
        };
        if matches.next().is_some() {
            return ReceiptResolveResult::Ambiguous;
        }
        ReceiptResolveResult::Found(ResolvedReceipt {
            path: format!("children[{index}]"),
            receipt,
        })
    }

    fn supplied_receipts<'a>(&'a self) -> Vec<ResolvedReceipt<'a>> {
        self.children
            .iter()
            .enumerate()
            .map(|(index, receipt)| ResolvedReceipt {
                path: format!("children[{index}]"),
                receipt,
            })
            .collect()
    }
}

fn referenced_receipt_id(reference: &Reference) -> Option<&str> {
    if reference.reference_type != ReferenceType::Receipt {
        return None;
    }
    reference
        .uri
        .strip_prefix("runx:receipt:")
        .filter(|id| !id.is_empty())
}

fn ordered_findings(verification: &ReceiptVerification) -> Vec<ExpectedFinding> {
    verification
        .findings
        .iter()
        .map(|finding| ExpectedFinding {
            code: format!("{:?}", finding.code),
            path: finding.path.clone(),
        })
        .collect()
}
