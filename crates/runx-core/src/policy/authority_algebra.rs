use runx_contracts::Reference;

#[must_use]
pub fn same_reference_address(child: &Reference, parent: &Reference) -> bool {
    child.reference_type == parent.reference_type && child.uri == parent.uri
}

#[must_use]
pub fn items_subset<T: PartialEq>(child: &[T], parent: &[T]) -> bool {
    child.iter().all(|item| parent.contains(item))
}

#[must_use]
pub fn parent_items_preserved<T: PartialEq>(child: &[T], parent: &[T]) -> bool {
    parent.iter().all(|item| child.contains(item))
}

#[must_use]
pub fn optional_ref_bound_subset<T: Ord>(child: Option<&T>, parent: Option<&T>) -> bool {
    match (child, parent) {
        (Some(child), Some(parent)) => child <= parent,
        (None, Some(_)) => false,
        (Some(_), None) | (None, None) => true,
    }
}

/// Whether a parent pattern covers a child value or pattern. `*` covers
/// everything, `<prefix>*` covers anything starting with the prefix (including
/// a narrower pattern), and anything else covers only itself.
#[must_use]
pub fn pattern_covers(parent: &str, child: &str) -> bool {
    match parent.strip_suffix('*') {
        Some(prefix) => child.starts_with(prefix),
        None => parent == child,
    }
}

/// Every child pattern is covered by some parent pattern. An empty parent
/// covers nothing, so a bound is granted only when it is listed.
#[must_use]
pub fn patterns_subset<T: AsRef<str>>(child: &[T], parent: &[T]) -> bool {
    child.iter().all(|child| {
        parent
            .iter()
            .any(|parent| pattern_covers(parent.as_ref(), child.as_ref()))
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::Strategy;

    use super::{
        items_subset, optional_ref_bound_subset, parent_items_preserved, pattern_covers,
        patterns_subset,
    };

    #[test]
    fn patterns_cover_exact_prefix_and_wildcard() {
        assert!(pattern_covers("*", "slack/channel.post"));
        assert!(pattern_covers("slack/*", "slack/channel.post"));
        assert!(pattern_covers("slack/*", "slack/channel.*"));
        assert!(!pattern_covers("slack/*", "github/issue.read"));
        assert!(!pattern_covers("slack/channel.*", "slack/*"));
        assert!(pattern_covers("slack://acme/C1", "slack://acme/C1"));
        assert!(!pattern_covers("slack://acme/C1", "slack://acme/C2"));
        assert!(patterns_subset(&["a/x", "a/y"], &["a/*"]));
        assert!(!patterns_subset(&["a/x"], &[] as &[&str]));
        assert!(patterns_subset(&[] as &[&str], &[] as &[&str]));
    }

    #[test]
    fn item_subset_is_reflexive() {
        let values = ["read", "write", "verify"];

        assert!(items_subset(&values, &values));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        #[test]
        fn item_subset_is_transitive(
            parent in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
            middle in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
            child in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
        ) {
            proptest::prop_assert!(
                !items_subset(&middle, &parent)
                    || !items_subset(&child, &middle)
                    || items_subset(&child, &parent)
            );
        }

        #[test]
        fn item_subset_denies_widening(
            parent in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
            missing in proptest::prelude::any::<u8>(),
        ) {
            proptest::prop_assume!(!parent.contains(&missing));
            let mut child = parent.clone();
            child.push(missing);
            proptest::prop_assert!(!items_subset(&child, &parent));
        }

        #[test]
        fn optional_bounds_allow_narrower_and_deny_wider_or_missing(
            (child, parent) in (1_u64..100_000).prop_flat_map(|child| (proptest::prelude::Just(child), 0_u64..child)),
        ) {
            proptest::prop_assert!(optional_ref_bound_subset(Some(&parent), Some(&child)));
            proptest::prop_assert!(!optional_ref_bound_subset(Some(&child), Some(&parent)));
            proptest::prop_assert!(!optional_ref_bound_subset(None, Some(&parent)));
            proptest::prop_assert!(optional_ref_bound_subset(Some(&child), None));
        }
    }

    #[test]
    fn parent_items_are_preserved_when_child_keeps_parent_requirements() {
        let parent = ["approval", "mfa"];
        let child = ["approval", "mfa", "reason"];

        assert!(parent_items_preserved(&child, &parent));
        assert!(!parent_items_preserved(&["approval"], &parent));
    }
}
