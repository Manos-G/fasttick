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

/// Short descriptions of the built-in nodes, sorted by ID. The texts are
/// written for FastTick from the behaviour of the BT.CPP 4.9.0 source.
const DESCRIPTIONS: &[(&str, &str)] = &[
    ("AlwaysFailure", "Returns FAILURE at once."),
    ("AlwaysSuccess", "Returns SUCCESS at once."),
    (
        "AsyncFallback",
        "A Fallback that returns RUNNING after each child that fails, so the \
         next child starts on the next tick.",
    ),
    (
        "AsyncSequence",
        "A Sequence that returns RUNNING after each child that succeeds, so \
         the next child starts on the next tick.",
    ),
    (
        "Delay",
        "Waits delay_msec milliseconds (it returns RUNNING), then ticks its \
         child and returns the result of the child.",
    ),
    (
        "Fallback",
        "Ticks its children in order until one of them does not fail. \
         Returns SUCCESS when a child succeeds, FAILURE when all children \
         fail.",
    ),
    (
        "ForceFailure",
        "Ticks its child and returns FAILURE when the child is complete.",
    ),
    (
        "ForceSuccess",
        "Ticks its child and returns SUCCESS when the child is complete.",
    ),
    (
        "IfThenElse",
        "The first child is the condition. If it succeeds, the node ticks the \
         second child; if it fails, the third child (or returns FAILURE when \
         there is no third child). Not reactive.",
    ),
    (
        "Inverter",
        "Ticks its child and changes SUCCESS to FAILURE and FAILURE to \
         SUCCESS.",
    ),
    (
        "KeepRunningUntilFailure",
        "Ticks its child again and again and returns RUNNING while the child \
         succeeds. Returns FAILURE when the child fails.",
    ),
    (
        "LoopBool",
        "Takes the first value from the queue in the port queue, writes it \
         to the port value and ticks the child. Continues while the queue has \
         values.",
    ),
    (
        "LoopDouble",
        "Takes the first value from the queue in the port queue, writes it \
         to the port value and ticks the child. Continues while the queue has \
         values.",
    ),
    (
        "LoopInt",
        "Takes the first value from the queue in the port queue, writes it \
         to the port value and ticks the child. Continues while the queue has \
         values.",
    ),
    (
        "LoopString",
        "Takes the first value from the queue in the port queue, writes it \
         to the port value and ticks the child. Continues while the queue has \
         values.",
    ),
    (
        "Parallel",
        "Ticks all children at the same time (in one thread). Returns \
         SUCCESS when success_count children succeed, FAILURE when \
         failure_count children fail (-1 means all children). Then it stops \
         the other children.",
    ),
    (
        "ParallelAll",
        "Ticks all children at the same time (in one thread) until all of \
         them are complete. Returns FAILURE if max_failures or more children \
         failed, else SUCCESS.",
    ),
    (
        "Precondition",
        "Ticks its child only if the script in the port if is true. Else it \
         returns the status in the port else.",
    ),
    (
        "ReactiveFallback",
        "A Fallback that ticks the children before the RUNNING child again on \
         each tick. If one of them succeeds, it stops the RUNNING child.",
    ),
    (
        "ReactiveSequence",
        "A Sequence that ticks the children before the RUNNING child again on \
         each tick. If one of them fails, it stops the RUNNING child.",
    ),
    (
        "Repeat",
        "Ticks its child num_cycles times while the child succeeds (-1 means \
         no limit). Returns FAILURE when the child fails.",
    ),
    (
        "RetryUntilSuccessful",
        "Ticks its child again when it fails, at most num_attempts times (-1 \
         means no limit). Returns SUCCESS when the child succeeds.",
    ),
    (
        "RunOnce",
        "Ticks its child until the child is complete, one time only. After \
         that it returns SKIPPED (then_skip true) or the first result of the \
         child.",
    ),
    (
        "Script",
        "Runs the script in the port code (for example a := 1; b := a + 2) \
         and returns SUCCESS.",
    ),
    (
        "ScriptCondition",
        "Returns SUCCESS if the script in the port code is true, else \
         FAILURE.",
    ),
    (
        "Sequence",
        "Ticks its children in order while they succeed. Returns FAILURE \
         when a child fails, SUCCESS when all children succeed. After a \
         failure it starts again from the first child.",
    ),
    (
        "SequenceWithMemory",
        "A Sequence that does not tick the children that succeeded again: \
         after a failure it continues from the child that failed.",
    ),
    (
        "SetBlackboard",
        "Writes the value of the port value to the blackboard entry in the \
         port output_key and returns SUCCESS.",
    ),
    (
        "SkipUnlessUpdated",
        "Ticks its child only if the blackboard entry in the port entry \
         changed since the last tick. Else it returns SKIPPED.",
    ),
    (
        "Sleep",
        "Returns RUNNING for msec milliseconds, then SUCCESS.",
    ),
    (
        "SubTree",
        "Runs another tree. The attributes connect the ports of that tree \
         to the keys of this tree (remapping).",
    ),
    (
        "Switch2",
        "Compares the port variable with case_1 and case_2 and ticks the \
         child of the first case that is equal. The last child is the \
         default.",
    ),
    (
        "Switch3",
        "Compares the port variable with case_1 to case_3 and ticks the \
         child of the first case that is equal. The last child is the \
         default.",
    ),
    (
        "Switch4",
        "Compares the port variable with case_1 to case_4 and ticks the \
         child of the first case that is equal. The last child is the \
         default.",
    ),
    (
        "Switch5",
        "Compares the port variable with case_1 to case_5 and ticks the \
         child of the first case that is equal. The last child is the \
         default.",
    ),
    (
        "Switch6",
        "Compares the port variable with case_1 to case_6 and ticks the \
         child of the first case that is equal. The last child is the \
         default.",
    ),
    (
        "Timeout",
        "Ticks its child. If the child is not complete after msec \
         milliseconds, it stops the child and returns FAILURE.",
    ),
    (
        "TryCatch",
        "Ticks the children before the last one as a Sequence. If one of \
         them fails, it ticks the last child (the catch child) and returns \
         FAILURE.",
    ),
    (
        "UnsetBlackboard",
        "Removes the blackboard entry in the port key and returns SUCCESS.",
    ),
    (
        "WaitValueUpdate",
        "Returns RUNNING until the blackboard entry in the port entry \
         changes, then ticks its child.",
    ),
    (
        "WasEntryUpdated",
        "Returns SUCCESS if the blackboard entry in the port entry changed \
         since the last tick, else FAILURE.",
    ),
    (
        "WhileDoElse",
        "The first child is the condition, ticked on each tick. While it \
         succeeds, the node ticks the second child; while it fails, the \
         third child. A change stops the RUNNING child.",
    ),
];

/// A short description of a built-in node, in simple English.
pub fn description(id: &str) -> Option<&'static str> {
    DESCRIPTIONS
        .binary_search_by(|(key, _)| (*key).cmp(id))
        .ok()
        .map(|index| DESCRIPTIONS[index].1)
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

    #[test]
    fn each_builtin_node_has_a_description() {
        for pair in DESCRIPTIONS.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{} >= {}", pair[0].0, pair[1].0);
        }
        assert_eq!(DESCRIPTIONS.len(), models().len());
        for model in models() {
            let text = description(&model.id).unwrap_or_else(|| panic!("{}", model.id));
            assert!(text.ends_with('.'), "{text}");
        }
        assert_eq!(description("MyAction"), None);
    }
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
