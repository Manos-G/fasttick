//! Include and exclude filters for transitions.
//!
//! A [`TransitionFilter`] has four parts. A transition passes the filter if
//! all parts accept it:
//!
//! - `include`: patterns. The node must match at least one pattern. An
//!   empty list accepts all nodes.
//! - `exclude`: patterns. The node must not match any pattern. Exclude
//!   wins over include.
//! - `statuses`: the new status must be in this set (if set).
//! - `time_range`: the time must be in this inclusive range (if set).
//!
//! A [`Pattern`] matches the node name, the node path or the node type ID.
//! The match ignores case. `*` matches any text (also no text) and `?`
//! matches one character. A pattern with wildcards must match the full
//! text. A pattern without wildcards matches any part of the text. Spaces
//! at the start and end of a pattern are removed; an empty pattern is
//! ignored.
//!
//! A transition with a UID that is not in the tree has no name, path or
//! type. It does not match any pattern.

use std::collections::{BTreeSet, HashMap};

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use serde::{Deserialize, Serialize};

use crate::btlog::Transition;
use crate::ordered_range;

/// A filter for transitions. See the module documentation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionFilter {
    /// Include patterns. Empty means all nodes.
    pub include: Vec<String>,
    /// Exclude patterns.
    pub exclude: Vec<String>,
    /// The accepted statuses. `None` means all statuses.
    pub statuses: Option<BTreeSet<NodeStatus>>,
    /// The accepted inclusive time range (offsets in microseconds). `None`
    /// means all times.
    pub time_range: Option<(u64, u64)>,
}

impl TransitionFilter {
    /// A filter that accepts all transitions.
    pub fn new() -> TransitionFilter {
        TransitionFilter::default()
    }

    /// True if the filter accepts all transitions.
    pub fn is_empty(&self) -> bool {
        self.include.iter().all(|pattern| pattern.trim().is_empty())
            && self.exclude.iter().all(|pattern| pattern.trim().is_empty())
            && self.statuses.is_none()
            && self.time_range.is_none()
    }

    /// Prepares the filter for a tree. The result decides fast for each
    /// transition.
    pub fn compile(&self, tree: &RuntimeTree) -> CompiledFilter {
        let include = compile_patterns(&self.include);
        let exclude = compile_patterns(&self.exclude);
        let node_match = |texts: &[&str]| {
            let included =
                include.is_empty() || include.iter().any(|pattern| pattern.matches_any(texts));
            included && !exclude.iter().any(|pattern| pattern.matches_any(texts))
        };
        let mut nodes = HashMap::new();
        for (position, node) in tree.nodes.iter().enumerate() {
            if let Some(uid) = node.uid {
                let path = tree.node_path(fasttick_core::runtime::NodeIndex(position));
                let texts = [node.name.as_str(), path.as_str(), node.type_id.as_str()];
                nodes.insert(uid, node_match(&texts));
            }
        }
        CompiledFilter {
            nodes,
            unknown_accepted: include.is_empty(),
            statuses: self
                .statuses
                .as_ref()
                .map(|set| NodeStatus::ALL.map(|status| set.contains(&status))),
            time_range: self.time_range.map(ordered_range),
        }
    }

    /// The positions in `transitions` of the transitions that pass the
    /// filter, in increasing order.
    pub fn apply(&self, tree: &RuntimeTree, transitions: &[Transition]) -> Vec<usize> {
        let compiled = self.compile(tree);
        transitions
            .iter()
            .enumerate()
            .filter(|(_, transition)| compiled.matches(transition))
            .map(|(position, _)| position)
            .collect()
    }
}

fn compile_patterns(patterns: &[String]) -> Vec<Pattern> {
    patterns
        .iter()
        .filter_map(|pattern| Pattern::new(pattern))
        .collect()
}

/// A [`TransitionFilter`] prepared for one tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledFilter {
    /// For each UID of the tree: true if the node passes the patterns.
    nodes: HashMap<Uid, bool>,
    /// True if a UID that is not in the tree passes the patterns.
    unknown_accepted: bool,
    /// For each status value (0 to 4): true if it passes.
    statuses: Option<[bool; 5]>,
    time_range: Option<(u64, u64)>,
}

impl CompiledFilter {
    /// True if the node passes the include and exclude patterns.
    pub fn node_matches(&self, uid: Uid) -> bool {
        self.nodes
            .get(&uid)
            .copied()
            .unwrap_or(self.unknown_accepted)
    }

    /// True if the transition passes the filter.
    pub fn matches(&self, transition: &Transition) -> bool {
        if let Some((start, end)) = self.time_range
            && (transition.offset_us < start || transition.offset_us > end)
        {
            return false;
        }
        if let Some(statuses) = &self.statuses
            && !statuses[usize::from(transition.status.as_u8())]
        {
            return false;
        }
        self.node_matches(transition.uid)
    }
}

/// A text pattern with the wildcards `*` and `?`. See the module
/// documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The lowercase characters of the pattern.
    chars: Vec<char>,
    /// True if the pattern has `*` or `?`.
    wildcard: bool,
}

impl Pattern {
    /// Makes a pattern. Returns `None` for an empty pattern (or only
    /// spaces).
    pub fn new(text: &str) -> Option<Pattern> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
        let wildcard = chars.iter().any(|c| *c == '*' || *c == '?');
        Some(Pattern { chars, wildcard })
    }

    /// True if the pattern matches the text.
    pub fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
        if self.wildcard {
            glob_match(&self.chars, &text)
        } else {
            contains(&text, &self.chars)
        }
    }

    /// True if the pattern matches one of the texts.
    pub fn matches_any(&self, texts: &[&str]) -> bool {
        texts.iter().any(|text| self.matches(text))
    }
}

fn contains(text: &[char], part: &[char]) -> bool {
    part.is_empty() || text.windows(part.len()).any(|window| window == part)
}

/// Matches a full text with `*` and `?`. The cost is `O(n * m)` at most.
fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    // The position after the last `*` and the text position it matched to.
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p + 1, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star_p, star_t)) => {
                    // Let the last `*` take one more character.
                    p = star_p;
                    t = star_t + 1;
                    backtrack = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> bool {
        Pattern::new(pattern).unwrap().matches(text)
    }

    #[test]
    fn substring_without_wildcards() {
        assert!(matches("move", "MoveBase"));
        assert!(matches("BASE", "MoveBase"));
        assert!(!matches("basex", "MoveBase"));
        assert!(matches(" base ", "MoveBase"));
    }

    #[test]
    fn wildcards_match_full_text() {
        assert!(matches("move*", "MoveBase"));
        assert!(!matches("base*", "MoveBase"));
        assert!(matches("*base", "MoveBase"));
        assert!(matches("*", ""));
        assert!(matches("*", "x"));
        assert!(matches("m?ve*", "MoveBase"));
        assert!(!matches("m?ve", "MoveBase"));
        assert!(matches("*o*a*", "MoveBase"));
        assert!(matches("a*b*c", "aXbYbZc"));
        assert!(!matches("a*b*c", "aXbYbZ"));
        assert!(matches("??", "ab"));
        assert!(!matches("??", "abc"));
        assert!(matches("**a**", "bab"));
    }

    #[test]
    fn unicode_case() {
        assert!(matches("ÄRGER", "kein ärger"));
        assert!(matches("ä?", "Äx"));
    }

    #[test]
    fn empty_patterns_are_ignored() {
        assert!(Pattern::new("").is_none());
        assert!(Pattern::new("   ").is_none());
        let filter = TransitionFilter {
            include: vec![" ".into()],
            ..TransitionFilter::default()
        };
        assert!(filter.is_empty());
    }
}
