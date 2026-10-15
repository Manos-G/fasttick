//! Port values: literals and blackboard references.

use std::borrow::Cow;
use std::fmt;

/// The kind of a port value (an attribute value of a port or a SubTree
/// remapping).
///
/// [`PortValue::parse`] follows `TreeNode::getRemappedKey` and
/// `TreeNode::isBlackboardPointer` (`src/tree_node.cpp`), and the `@`
/// prefix of `Blackboard::getEntry` (`src/blackboard.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortValue<'a> {
    /// A literal value. BT.CPP converts the text to the port type
    /// (`convertFromString`).
    Literal(&'a str),
    /// A blackboard reference `{key}`.
    BlackboardRef {
        /// The key between the braces, without the `@` prefix. BT.CPP
        /// removes spaces around the braces, but not inside them: the key
        /// of `{ a }` is ` a `.
        key: &'a str,
        /// True if the key starts with `@`: the entry is in the root
        /// blackboard of the tree (`{@key}`).
        root_scope: bool,
    },
    /// `{=}`: the blackboard entry has the same name as the port.
    SameName,
}

/// Returns the text between the braces if `value` is a blackboard pointer.
///
/// Copy of `TreeNode::isBlackboardPointer` (`src/tree_node.cpp`): spaces
/// (only the character U+0020) before `{` and after `}` are ignored, and the
/// value has at least three characters (`{x}`).
pub fn blackboard_pointer(value: &str) -> Option<&str> {
    let trimmed = value.trim_matches(' ');
    if trimmed.len() >= 3 && trimmed.starts_with('{') && trimmed.ends_with('}') {
        Some(&trimmed[1..trimmed.len() - 1])
    } else {
        None
    }
}

impl<'a> PortValue<'a> {
    /// Parses the value of a port of a normal node.
    ///
    /// - `{=}` and `=` give [`PortValue::SameName`] (`getRemappedKey` and
    ///   `setOutput` accept both).
    /// - A blackboard pointer (see [`blackboard_pointer`]) gives
    ///   [`PortValue::BlackboardRef`]. A key that starts with `@` refers to
    ///   the root blackboard.
    /// - All other values are literals.
    pub fn parse(value: &'a str) -> PortValue<'a> {
        if value == "{=}" || value == "=" {
            return PortValue::SameName;
        }
        Self::parse_pointer_or_literal(value)
    }

    /// Parses the value of a remapping attribute of a `<SubTree>` node.
    ///
    /// `recursivelyCreateSubtree` (`src/xml_parsing.cpp`) changes only
    /// `{=}` to `{port}`. A plain `=` is a literal there: BT.CPP writes the
    /// text `=` into the blackboard of the SubTree.
    pub fn parse_remapping(value: &'a str) -> PortValue<'a> {
        if value == "{=}" {
            return PortValue::SameName;
        }
        Self::parse_pointer_or_literal(value)
    }

    fn parse_pointer_or_literal(value: &'a str) -> PortValue<'a> {
        match blackboard_pointer(value) {
            Some(key) => match key.strip_prefix('@') {
                Some(rest) => PortValue::BlackboardRef {
                    key: rest,
                    root_scope: true,
                },
                None => PortValue::BlackboardRef {
                    key,
                    root_scope: false,
                },
            },
            None => PortValue::Literal(value),
        }
    }

    /// True for a blackboard reference or `{=}`.
    pub fn is_blackboard(&self) -> bool {
        !matches!(self, PortValue::Literal(_))
    }

    /// The blackboard key that the port uses, as BT.CPP gives it to the
    /// blackboard: with the `@` prefix for the root scope, and the port name
    /// for `{=}`. `None` for a literal.
    pub fn blackboard_key(&self, port_name: &'a str) -> Option<Cow<'a, str>> {
        match *self {
            PortValue::Literal(_) => None,
            PortValue::SameName => Some(Cow::Borrowed(port_name)),
            PortValue::BlackboardRef {
                key,
                root_scope: false,
            } => Some(Cow::Borrowed(key)),
            PortValue::BlackboardRef {
                key,
                root_scope: true,
            } => Some(Cow::Owned(format!("@{key}"))),
        }
    }
}

impl fmt::Display for PortValue<'_> {
    /// Writes the value in its normal form: `{key}`, `{@key}`, `{=}` or the
    /// literal text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortValue::Literal(text) => f.write_str(text),
            PortValue::BlackboardRef { key, root_scope } => {
                write!(f, "{{{}{key}}}", if *root_scope { "@" } else { "" })
            }
            PortValue::SameName => f.write_str("{=}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(key: &str, root_scope: bool) -> PortValue<'_> {
        PortValue::BlackboardRef { key, root_scope }
    }

    #[test]
    fn pointers() {
        assert_eq!(blackboard_pointer("{a}"), Some("a"));
        assert_eq!(blackboard_pointer("  {a}  "), Some("a"));
        assert_eq!(blackboard_pointer("{ a }"), Some(" a "));
        assert_eq!(blackboard_pointer("{}"), None);
        assert_eq!(blackboard_pointer(" {} "), None);
        assert_eq!(blackboard_pointer("   "), None);
        assert_eq!(blackboard_pointer(""), None);
        assert_eq!(blackboard_pointer("\t{a}"), None);
        assert_eq!(blackboard_pointer("{a"), None);
        assert_eq!(blackboard_pointer("a}"), None);
        assert_eq!(blackboard_pointer("x{a}"), None);
        assert_eq!(blackboard_pointer("{a}{b}"), Some("a}{b"));
        assert_eq!(blackboard_pointer("{ä}"), Some("ä"));
    }

    #[test]
    fn parse_node_values() {
        assert_eq!(PortValue::parse("{goal}"), bb("goal", false));
        assert_eq!(PortValue::parse("{@goal}"), bb("goal", true));
        assert_eq!(PortValue::parse(" {@goal} "), bb("goal", true));
        assert_eq!(PortValue::parse("{ @goal}"), bb(" @goal", false));
        assert_eq!(PortValue::parse("{@}"), bb("", true));
        assert_eq!(PortValue::parse("{=}"), PortValue::SameName);
        assert_eq!(PortValue::parse("="), PortValue::SameName);
        assert_eq!(PortValue::parse(" {=} "), bb("=", false));
        assert_eq!(PortValue::parse("42"), PortValue::Literal("42"));
        assert_eq!(PortValue::parse(""), PortValue::Literal(""));
        assert_eq!(PortValue::parse("{}"), PortValue::Literal("{}"));
        assert_eq!(PortValue::parse("@goal"), PortValue::Literal("@goal"));
        assert_eq!(PortValue::parse("1;2;3"), PortValue::Literal("1;2;3"));
    }

    #[test]
    fn parse_remapping_values() {
        assert_eq!(PortValue::parse_remapping("{=}"), PortValue::SameName);
        assert_eq!(PortValue::parse_remapping("="), PortValue::Literal("="));
        assert_eq!(PortValue::parse_remapping("{x}"), bb("x", false));
        assert_eq!(PortValue::parse_remapping("{@x}"), bb("x", true));
        assert_eq!(
            PortValue::parse_remapping("text"),
            PortValue::Literal("text")
        );
    }

    #[test]
    fn keys_and_display() {
        assert_eq!(
            PortValue::parse("{a}").blackboard_key("p").as_deref(),
            Some("a")
        );
        assert_eq!(
            PortValue::parse("{@a}").blackboard_key("p").as_deref(),
            Some("@a")
        );
        assert_eq!(
            PortValue::parse("{=}").blackboard_key("p").as_deref(),
            Some("p")
        );
        assert_eq!(PortValue::parse("lit").blackboard_key("p"), None);
        assert!(PortValue::parse("{a}").is_blackboard());
        assert!(PortValue::SameName.is_blackboard());
        assert!(!PortValue::parse("a").is_blackboard());
        for text in ["{a}", "{@a}", "{=}", "literal"] {
            assert_eq!(PortValue::parse(text).to_string(), text);
        }
        assert_eq!(PortValue::parse("  {a} ").to_string(), "{a}");
    }
}
