//! The receipt trees a set of receipts forms, found from the lineage each
//! parent signs.

use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::Receipt;

use super::findings::referenced_receipt_id;
use super::receipt_edge_references;

/// A receipt as the trees of a set see it: its id and the receipt ids its
/// signed lineage names. Grouping needs no more, so a store is grouped
/// without holding its receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptLinks {
    pub id: String,
    pub children: Vec<String>,
}

impl ReceiptLinks {
    #[must_use]
    pub fn of(receipt: &Receipt) -> Self {
        Self {
            id: receipt.id.to_string(),
            children: receipt_edge_references(receipt)
                .filter_map(referenced_receipt_id)
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// One tree among a set of receipts: its root's id and every receipt id it
/// reaches, the root's included.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceiptTreeGroup {
    pub root_id: String,
    pub member_ids: BTreeSet<String>,
}

/// The trees `receipts` form, by root id. Runtime receipts form an immutable
/// parent-to-child workflow: a parent names its children, and a child shared by
/// several roots belongs to each. A cycle has no natural root, so each
/// receipt no tree reaches anchors one, and tree verification reports it.
#[must_use]
pub fn group_receipt_trees(receipts: &[ReceiptLinks]) -> Vec<ReceiptTreeGroup> {
    let by_id: BTreeSet<&str> = receipts.iter().map(|receipt| receipt.id.as_str()).collect();
    let mut children_by_parent: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut parents_by_child: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for receipt in receipts {
        for child in &receipt.children {
            children_by_parent
                .entry(receipt.id.as_str())
                .or_default()
                .insert(child.as_str());
            parents_by_child
                .entry(child.as_str())
                .or_default()
                .insert(receipt.id.as_str());
        }
    }
    let reach = |root: &str| -> ReceiptTreeGroup {
        let mut member_ids = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(receipt_id) = pending.pop() {
            if !member_ids.insert(receipt_id.to_owned()) {
                continue;
            }
            if let Some(children) = children_by_parent.get(receipt_id) {
                pending.extend(children.iter().filter(|child| by_id.contains(*child)));
            }
        }
        ReceiptTreeGroup {
            root_id: root.to_owned(),
            member_ids,
        }
    };
    let mut trees = receipts
        .iter()
        .filter(|receipt| {
            parents_by_child
                .get(receipt.id.as_str())
                .is_none_or(|parents| parents.iter().all(|parent| !by_id.contains(parent)))
        })
        .map(|receipt| reach(receipt.id.as_str()))
        .collect::<Vec<_>>();
    let mut covered = trees
        .iter()
        .flat_map(|tree| tree.member_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    for receipt in receipts {
        if covered.contains(receipt.id.as_str()) {
            continue;
        }
        let tree = reach(receipt.id.as_str());
        covered.extend(tree.member_ids.iter().cloned());
        trees.push(tree);
    }
    trees.sort_by(|left, right| left.root_id.cmp(&right.root_id));
    trees
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::test_support::{SUCCESS_RECEIPT, child, fixture};
    use super::{ReceiptLinks, group_receipt_trees};

    /// A tree is its root and every receipt its lineage names; a receipt
    /// no lineage names roots a tree of its own.
    #[test]
    fn a_tree_is_its_root_and_the_children_its_lineage_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = fixture(SUCCESS_RECEIPT)?;
        let named = child("rcpt_child_1")?;
        let stray = child("rcpt_other")?;

        let trees = group_receipt_trees(&[
            ReceiptLinks::of(&root),
            ReceiptLinks::of(&named),
            ReceiptLinks::of(&stray),
        ]);

        assert_eq!(trees.len(), 2);
        let rooted = trees
            .iter()
            .find(|tree| tree.root_id == root.id.as_str())
            .ok_or("the root's tree is missing")?;
        assert_eq!(
            rooted.member_ids,
            BTreeSet::from([root.id.to_string(), named.id.to_string()])
        );
        assert!(trees.iter().any(|tree| tree.root_id == stray.id.as_str()));
        Ok(())
    }
}
