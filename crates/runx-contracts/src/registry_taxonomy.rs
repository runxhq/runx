//! The registry taxonomy: every category a skill or connector may declare.
//! `registry/taxonomy.json` defines it; `scripts/generate-registry-taxonomy.mjs`
//! projects it here and into `@runxhq/contracts`. A declared category must be
//! one of these ids and must apply to the kind of thing declaring it; anything
//! undeclared is stored as uncategorized, never guessed.

mod generated;

/// What declares a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxonomySubject {
    Skill,
    Connector,
}

impl TaxonomySubject {
    const fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Connector => "connector",
        }
    }
}

/// One category, as the taxonomy defines it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub skill: bool,
    pub connector: bool,
    /// Builder-facing work, shown apart from the domain row.
    pub builder: bool,
}

impl Category {
    #[must_use]
    pub const fn applies_to(&self, subject: TaxonomySubject) -> bool {
        match subject {
            TaxonomySubject::Skill => self.skill,
            TaxonomySubject::Connector => self.connector,
        }
    }
}

/// The value stored for a skill or connector that declares no category. Not
/// a category an author can declare.
pub const UNCATEGORIZED: &str = generated::UNCATEGORIZED;

/// Every declarable category, in rail order.
#[must_use]
pub const fn registry_categories() -> &'static [Category] {
    generated::CATEGORIES
}

#[must_use]
pub fn registry_category(id: &str) -> Option<&'static Category> {
    generated::CATEGORIES
        .iter()
        .find(|category| category.id == id)
}

/// Checks a declared category against the taxonomy.
///
/// # Errors
///
/// Returns a message when `id` is not a category this kind of thing can declare.
pub fn validate_registry_category(subject: TaxonomySubject, id: &str) -> Result<(), String> {
    match registry_category(id) {
        Some(category) if category.applies_to(subject) => Ok(()),
        Some(_) => Err(format!(
            "category {id:?} is not a {} category; {}",
            subject.label(),
            choices(subject)
        )),
        None => Err(format!(
            "category {id:?} is not in the registry taxonomy; {}",
            choices(subject)
        )),
    }
}

fn choices(subject: TaxonomySubject) -> String {
    let ids = generated::CATEGORIES
        .iter()
        .filter(|category| category.applies_to(subject))
        .map(|category| category.id)
        .collect::<Vec<_>>();
    format!("a {} declares one of {}", subject.label(), ids.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_categories_by_the_kind_that_declares_them() {
        assert_eq!(
            validate_registry_category(TaxonomySubject::Skill, "research"),
            Ok(())
        );
        assert_eq!(
            validate_registry_category(TaxonomySubject::Connector, "communication"),
            Ok(())
        );
        assert_eq!(
            validate_registry_category(TaxonomySubject::Skill, "payments"),
            Ok(())
        );
        assert_eq!(
            validate_registry_category(TaxonomySubject::Connector, "payments"),
            Ok(())
        );
        assert!(validate_registry_category(TaxonomySubject::Skill, "communication").is_err());
        assert!(validate_registry_category(TaxonomySubject::Connector, "research").is_err());
        assert!(validate_registry_category(TaxonomySubject::Skill, UNCATEGORIZED).is_err());
        assert!(validate_registry_category(TaxonomySubject::Skill, "Research").is_err());
    }
}
