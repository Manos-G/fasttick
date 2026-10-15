//! The category of a node type, for the colour band and the glyph.
//!
//! - Edit views use the model registry of the project
//!   ([`crate::canvas::ProjectCategories`]).
//! - Live and replay views use the `<TreeNodesModel>` that comes with the
//!   tree ([`TableCategories`], see `documents::runtime_categories`).
//! - [`BuiltinCategories`] knows the built-in nodes of BT.CPP 4.9.0. For a
//!   type without a model, the builder infers the category from the number
//!   of children.

use super::model::NodeCategory;

/// Gives the category of a node type ID.
pub trait CategoryLookup {
    /// The category of `type_id`, or `None` if no model is known.
    fn category(&self, type_id: &str) -> Option<NodeCategory>;
}

/// The built-in nodes of BT.CPP 4.9.0.
///
/// The list is the output of `bt4_nodes_model --include-builtin` (embedded
/// in `fasttick-core/resources/builtin_models_4.9.0.xml`); a unit test
/// checks that the two lists agree.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinCategories;

/// Built-in type IDs and their categories, sorted by ID.
const BUILTIN: &[(&str, NodeCategory)] = &[
    ("AlwaysFailure", NodeCategory::Action),
    ("AlwaysSuccess", NodeCategory::Action),
    ("AsyncFallback", NodeCategory::Control),
    ("AsyncSequence", NodeCategory::Control),
    ("Delay", NodeCategory::Decorator),
    ("Fallback", NodeCategory::Control),
    ("ForceFailure", NodeCategory::Decorator),
    ("ForceSuccess", NodeCategory::Decorator),
    ("IfThenElse", NodeCategory::Control),
    ("Inverter", NodeCategory::Decorator),
    ("KeepRunningUntilFailure", NodeCategory::Decorator),
    ("LoopBool", NodeCategory::Decorator),
    ("LoopDouble", NodeCategory::Decorator),
    ("LoopInt", NodeCategory::Decorator),
    ("LoopString", NodeCategory::Decorator),
    ("Parallel", NodeCategory::Control),
    ("ParallelAll", NodeCategory::Control),
    ("Precondition", NodeCategory::Decorator),
    ("ReactiveFallback", NodeCategory::Control),
    ("ReactiveSequence", NodeCategory::Control),
    ("Repeat", NodeCategory::Decorator),
    ("RetryUntilSuccessful", NodeCategory::Decorator),
    ("RunOnce", NodeCategory::Decorator),
    ("Script", NodeCategory::Action),
    ("ScriptCondition", NodeCategory::Condition),
    ("Sequence", NodeCategory::Control),
    ("SequenceWithMemory", NodeCategory::Control),
    ("SetBlackboard", NodeCategory::Action),
    ("SkipUnlessUpdated", NodeCategory::Decorator),
    ("Sleep", NodeCategory::Action),
    ("SubTree", NodeCategory::SubTree),
    ("Switch2", NodeCategory::Control),
    ("Switch3", NodeCategory::Control),
    ("Switch4", NodeCategory::Control),
    ("Switch5", NodeCategory::Control),
    ("Switch6", NodeCategory::Control),
    ("Timeout", NodeCategory::Decorator),
    ("TryCatch", NodeCategory::Control),
    ("UnsetBlackboard", NodeCategory::Action),
    ("WaitValueUpdate", NodeCategory::Decorator),
    ("WasEntryUpdated", NodeCategory::Action),
    ("WhileDoElse", NodeCategory::Control),
];

impl CategoryLookup for BuiltinCategories {
    fn category(&self, type_id: &str) -> Option<NodeCategory> {
        BUILTIN
            .binary_search_by(|(id, _)| (*id).cmp(type_id))
            .ok()
            .map(|index| BUILTIN[index].1)
    }
}

/// A lookup from a list of `(type ID, category)` pairs, with a fallback.
/// Useful for tests and for models that the user gives.
#[derive(Debug, Clone, Default)]
pub struct TableCategories {
    entries: std::collections::HashMap<String, NodeCategory>,
}

impl TableCategories {
    /// A table with these entries.
    pub fn new<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (S, NodeCategory)>,
        S: Into<String>,
    {
        TableCategories {
            entries: entries.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

impl CategoryLookup for TableCategories {
    fn category(&self, type_id: &str) -> Option<NodeCategory> {
        self.entries
            .get(type_id)
            .copied()
            .or_else(|| BuiltinCategories.category(type_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted() {
        for pair in BUILTIN.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} >= {}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn lookup() {
        assert_eq!(
            BuiltinCategories.category("Sequence"),
            Some(NodeCategory::Control)
        );
        assert_eq!(
            BuiltinCategories.category("ScriptCondition"),
            Some(NodeCategory::Condition)
        );
        assert_eq!(BuiltinCategories.category("MyAction"), None);
        let table = TableCategories::new([("MyCheck", NodeCategory::Condition)]);
        assert_eq!(table.category("MyCheck"), Some(NodeCategory::Condition));
        assert_eq!(table.category("Inverter"), Some(NodeCategory::Decorator));
    }

    /// The table agrees with the embedded model list of fasttick-core.
    #[test]
    fn table_matches_embedded_models() {
        let text = include_str!("../../../fasttick-core/resources/builtin_models_4.9.0.xml");
        let mut found = Vec::new();
        for category in ["Action", "Condition", "Control", "Decorator", "SubTree"] {
            let prefix = format!("<{category} ID=\"");
            for (start, _) in text.match_indices(&prefix) {
                let rest = &text[start + prefix.len()..];
                let id = &rest[..rest.find('"').unwrap()];
                found.push((id.to_owned(), NodeCategory::from_tag(category).unwrap()));
            }
        }
        found.sort();
        let expected: Vec<(String, NodeCategory)> = BUILTIN
            .iter()
            .map(|(id, category)| ((*id).to_owned(), *category))
            .collect();
        assert_eq!(found, expected);
    }
}
