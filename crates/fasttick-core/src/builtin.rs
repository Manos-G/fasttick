//! The built-in node models of BT.CPP 4.9.0 and the child-count rules.
//!
//! The models come from the embedded file
//! `resources/builtin_models_4.9.0.xml`. The FastTick test harness made it
//! with `BT::writeTreeNodesModelXML(factory, true)` (`bt4_nodes_model
//! --include-builtin`). It has the 42 IDs that the constructor of
//! `BehaviorTreeFactory` registers (`src/bt_factory.cpp`).
//!
//! BT.CPP 4.9.0 also has one alias for BT.CPP 3 code: `SequenceStar`, a
//! second registration of `SequenceWithMemory`. BT.CPP registers it only
//! when it is built with the CMake option `USE_V3_COMPATIBLE_NAMES`
//! (`#ifdef USE_BTCPP3_OLD_NAMES` in `src/bt_factory.cpp`; the option is
//! off by default). It is not in the embedded file. See [`V3_ALIASES`].

use std::fmt;
use std::sync::OnceLock;

use crate::models::{ModelSource, NodeKind, NodeModel, parse_models_text};

/// The text of the embedded model file.
pub const BUILTIN_MODELS_XML: &str = include_str!("../resources/builtin_models_4.9.0.xml");

/// The BT.CPP version of the built-in models.
pub const BTCPP_VERSION: &str = "4.9.0";

/// IDs that BT.CPP 4.9.0 registers only with `USE_V3_COMPATIBLE_NAMES`:
/// pairs of (alias, built-in ID with the same node class).
pub const V3_ALIASES: &[(&str, &str)] = &[("SequenceStar", "SequenceWithMemory")];

/// The built-in models, sorted by ID, with [`ModelSource::Builtin`].
pub fn models() -> &'static [NodeModel] {
    static MODELS: OnceLock<Vec<NodeModel>> = OnceLock::new();
    MODELS.get_or_init(|| {
        // The embedded text is fixed and tested; an error gives no models.
        parse_models_text(BUILTIN_MODELS_XML, ModelSource::Builtin)
            .map(|parsed| parsed.into_models())
            .unwrap_or_default()
    })
}

/// The built-in model with this ID.
pub fn model(id: &str) -> Option<&'static NodeModel> {
    models().iter().find(|m| m.id == id)
}

/// True if BT.CPP 4.9.0 registers this ID by default.
pub fn is_builtin(id: &str) -> bool {
    model(id).is_some()
}

/// The built-in ID for a BT.CPP 3 alias (see [`V3_ALIASES`]).
pub fn v3_alias_target(id: &str) -> Option<&'static str> {
    V3_ALIASES
        .iter()
        .find(|(alias, _)| *alias == id)
        .map(|(_, target)| *target)
}

/// The number of child elements that a node can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChildRule {
    /// Exactly this number.
    Exactly(usize),
    /// This number or more.
    AtLeast(usize),
    /// From `min` to `max` (both included).
    Between {
        /// The smallest number.
        min: usize,
        /// The largest number.
        max: usize,
    },
}

impl ChildRule {
    /// True if `count` children obey the rule.
    pub fn allows(self, count: usize) -> bool {
        match self {
            ChildRule::Exactly(n) => count == n,
            ChildRule::AtLeast(n) => count >= n,
            ChildRule::Between { min, max } => (min..=max).contains(&count),
        }
    }

    /// The smallest allowed number of children.
    pub fn min(self) -> usize {
        match self {
            ChildRule::Exactly(n) | ChildRule::AtLeast(n) => n,
            ChildRule::Between { min, .. } => min,
        }
    }

    /// The largest allowed number of children, or `None` if there is no
    /// limit.
    pub fn max(self) -> Option<usize> {
        match self {
            ChildRule::Exactly(n) => Some(n),
            ChildRule::AtLeast(_) => None,
            ChildRule::Between { max, .. } => Some(max),
        }
    }

    /// True if one more child obeys the rule after `count` children (for
    /// example, to allow a drop on the canvas).
    pub fn accepts_more(self, count: usize) -> bool {
        self.max().is_none_or(|max| count < max)
    }
}

impl fmt::Display for ChildRule {
    /// Writes the rule as text, for example "exactly 1 child".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = |n: usize| if n == 1 { "child" } else { "children" };
        match *self {
            ChildRule::Exactly(0) => f.write_str("no children"),
            ChildRule::Exactly(n) => write!(f, "exactly {n} {}", plural(n)),
            ChildRule::AtLeast(n) => write!(f, "{n} or more children"),
            ChildRule::Between { min, max } => write!(f, "{min} to {max} children"),
        }
    }
}

/// The child rule of a node kind, from `VerifyXML` (`src/xml_parsing.cpp`):
///
/// - Action and Condition: no children ("must not have any child").
/// - Decorator: exactly 1 ("must have exactly 1 child").
/// - Control: 1 or more ("must have 1 or more children").
/// - SubTree: no children ("should not have any child").
pub fn kind_child_rule(kind: NodeKind) -> ChildRule {
    match kind {
        NodeKind::Action | NodeKind::Condition | NodeKind::SubTree => ChildRule::Exactly(0),
        NodeKind::Decorator => ChildRule::Exactly(1),
        NodeKind::Control => ChildRule::AtLeast(1),
    }
}

/// The child rule of a built-in ID that has its own rule, or `None`:
///
/// - `TryCatch`: 2 or more (`VerifyXML`; `TryCatchNode::tick` in
///   `src/controls/try_catch_node.cpp`).
/// - `IfThenElse`: 2 or 3 (`IfThenElseNode::tick` in
///   `src/controls/if_then_else_node.cpp`).
/// - `WhileDoElse`: 2 or 3 (`WhileDoElseNode::tick` in
///   `src/controls/while_do_else_node.cpp`).
/// - `Switch2` to `Switch6`: exactly N + 1 (`SwitchNode<N>::tick` in
///   `include/behaviortree_cpp/controls/switch_node.h`).
///
/// BT.CPP checks the last three only when the node ticks, not when it
/// loads the XML. The rule applies to the built-in IDs only: a custom
/// registration with the same class can have any ID.
pub fn special_child_rule(type_id: &str) -> Option<ChildRule> {
    match type_id {
        "TryCatch" => Some(ChildRule::AtLeast(2)),
        "IfThenElse" | "WhileDoElse" => Some(ChildRule::Between { min: 2, max: 3 }),
        _ => {
            let n: usize = type_id.strip_prefix("Switch")?.parse().ok()?;
            ((2..=6).contains(&n) && type_id.len() == "SwitchN".len())
                .then_some(ChildRule::Exactly(n + 1))
        }
    }
}

/// The child rule of a node: the rule of the built-in ID if it has one,
/// else the rule of its kind. `None` if the kind is not known.
///
/// A `SequenceStar` alias has the rule of `SequenceWithMemory`.
pub fn child_rule(type_id: &str, kind: Option<NodeKind>) -> Option<ChildRule> {
    let id = v3_alias_target(type_id).unwrap_or(type_id);
    special_child_rule(id).or_else(|| kind.map(kind_child_rule))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PortDirection, parse_models_text};

    #[test]
    fn forty_two_models() {
        let all = models();
        assert_eq!(all.len(), 42);
        let parsed = parse_models_text(BUILTIN_MODELS_XML, ModelSource::Builtin).unwrap();
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        let mut ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
        let sorted = {
            let mut s = ids.clone();
            s.sort();
            s
        };
        assert_eq!(ids, sorted);
        ids.dedup();
        assert_eq!(ids.len(), 42);
        assert!(all.iter().all(|m| m.source == ModelSource::Builtin));
        // The registrations of the BehaviorTreeFactory constructor.
        for id in [
            "Fallback",
            "AsyncFallback",
            "Sequence",
            "AsyncSequence",
            "SequenceWithMemory",
            "Parallel",
            "ParallelAll",
            "ReactiveSequence",
            "ReactiveFallback",
            "IfThenElse",
            "WhileDoElse",
            "TryCatch",
            "Inverter",
            "RetryUntilSuccessful",
            "KeepRunningUntilFailure",
            "Repeat",
            "Timeout",
            "Delay",
            "RunOnce",
            "ForceSuccess",
            "ForceFailure",
            "AlwaysSuccess",
            "AlwaysFailure",
            "Script",
            "ScriptCondition",
            "SetBlackboard",
            "Sleep",
            "UnsetBlackboard",
            "SubTree",
            "Precondition",
            "Switch2",
            "Switch3",
            "Switch4",
            "Switch5",
            "Switch6",
            "LoopInt",
            "LoopBool",
            "LoopDouble",
            "LoopString",
            "WasEntryUpdated",
            "SkipUnlessUpdated",
            "WaitValueUpdate",
        ] {
            assert!(is_builtin(id), "{id}");
        }
        assert!(!is_builtin("SequenceStar"));
    }

    #[test]
    fn model_details() {
        let parallel = model("Parallel").unwrap();
        assert_eq!(parallel.kind, NodeKind::Control);
        let success = parallel.port("success_count").unwrap();
        assert_eq!(success.type_name.as_deref(), Some("int"));
        assert_eq!(success.default.as_deref(), Some("-1"));
        assert!(success.description.as_deref().unwrap().contains("succeed"));
        let loop_int = model("LoopInt").unwrap();
        assert_eq!(
            loop_int.port("queue").unwrap().direction,
            PortDirection::InOut
        );
        assert_eq!(
            loop_int.port("value").unwrap().direction,
            PortDirection::Output
        );
        let subtree = model("SubTree").unwrap();
        assert_eq!(subtree.kind, NodeKind::SubTree);
        assert_eq!(
            subtree.port("_autoremap").unwrap().default.as_deref(),
            Some("false")
        );
        let sleep = model("Sleep").unwrap();
        assert!(sleep.port("msec").unwrap().is_required());
        assert_eq!(model("ScriptCondition").unwrap().kind, NodeKind::Condition);
        assert_eq!(model("Switch6").unwrap().ports.len(), 7);
    }

    #[test]
    fn child_rules() {
        assert_eq!(kind_child_rule(NodeKind::Action), ChildRule::Exactly(0));
        assert_eq!(kind_child_rule(NodeKind::Condition), ChildRule::Exactly(0));
        assert_eq!(kind_child_rule(NodeKind::SubTree), ChildRule::Exactly(0));
        assert_eq!(kind_child_rule(NodeKind::Decorator), ChildRule::Exactly(1));
        assert_eq!(kind_child_rule(NodeKind::Control), ChildRule::AtLeast(1));
        assert_eq!(special_child_rule("TryCatch"), Some(ChildRule::AtLeast(2)));
        assert_eq!(
            special_child_rule("IfThenElse"),
            Some(ChildRule::Between { min: 2, max: 3 })
        );
        assert_eq!(
            special_child_rule("WhileDoElse"),
            Some(ChildRule::Between { min: 2, max: 3 })
        );
        for n in 2..=6 {
            assert_eq!(
                special_child_rule(&format!("Switch{n}")),
                Some(ChildRule::Exactly(n + 1))
            );
        }
        for id in [
            "Switch1", "Switch7", "Switch", "Switch02", "Switch+3", "Sequence",
        ] {
            assert_eq!(special_child_rule(id), None, "{id}");
        }
        assert_eq!(
            child_rule("Sequence", Some(NodeKind::Control)),
            Some(ChildRule::AtLeast(1))
        );
        assert_eq!(child_rule("Switch3", None), Some(ChildRule::Exactly(4)));
        assert_eq!(child_rule("Unknown", None), None);
        assert_eq!(
            child_rule("SequenceStar", Some(NodeKind::Control)),
            Some(ChildRule::AtLeast(1))
        );
        assert_eq!(v3_alias_target("SequenceStar"), Some("SequenceWithMemory"));
        assert_eq!(v3_alias_target("Sequence"), None);
        // Every built-in model has a rule.
        for m in models() {
            assert!(child_rule(&m.id, Some(m.kind)).is_some());
        }
    }

    #[test]
    fn rule_checks_and_text() {
        let r = ChildRule::Between { min: 2, max: 3 };
        assert!(!r.allows(1) && r.allows(2) && r.allows(3) && !r.allows(4));
        assert_eq!((r.min(), r.max()), (2, Some(3)));
        assert!(r.accepts_more(2) && !r.accepts_more(3));
        let r = ChildRule::AtLeast(1);
        assert!(!r.allows(0) && r.allows(100));
        assert!(r.accepts_more(1000));
        assert_eq!(r.max(), None);
        let r = ChildRule::Exactly(1);
        assert!(r.accepts_more(0) && !r.accepts_more(1));
        assert_eq!(ChildRule::Exactly(0).to_string(), "no children");
        assert_eq!(ChildRule::Exactly(1).to_string(), "exactly 1 child");
        assert_eq!(ChildRule::Exactly(4).to_string(), "exactly 4 children");
        assert_eq!(ChildRule::AtLeast(1).to_string(), "1 or more children");
        assert_eq!(r.to_string(), "exactly 1 child");
        assert_eq!(
            ChildRule::Between { min: 2, max: 3 }.to_string(),
            "2 to 3 children"
        );
    }
}
