//! Attribute classes of a tree node element.

use std::fmt;

use super::names::{POST_CONDITION_NAMES, PRE_CONDITION_NAMES, is_allowed_port_name};

/// A pre-condition attribute. BT.CPP evaluates the pre-conditions in the
/// order of [`PreCondition::ALL`] (`enum class PreCond` and
/// `PreCondNames` in `include/behaviortree_cpp/tree_node.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PreCondition {
    /// `_failureIf`: if the script is true, return FAILURE without a tick.
    FailureIf,
    /// `_successIf`: if the script is true, return SUCCESS without a tick.
    SuccessIf,
    /// `_skipIf`: if the script is true, return SKIPPED without a tick.
    SkipIf,
    /// `_while`: if the script is false, return SKIPPED (or halt the node
    /// if it is running).
    While,
}

impl PreCondition {
    /// All pre-conditions, in the BT.CPP order.
    pub const ALL: [PreCondition; 4] = [
        PreCondition::FailureIf,
        PreCondition::SuccessIf,
        PreCondition::SkipIf,
        PreCondition::While,
    ];

    /// The attribute name, for example `_skipIf`.
    pub fn attribute_name(self) -> &'static str {
        PRE_CONDITION_NAMES[self as usize]
    }

    /// The pre-condition for an attribute name.
    pub fn from_attribute_name(name: &str) -> Option<PreCondition> {
        PRE_CONDITION_NAMES
            .iter()
            .position(|&n| n == name)
            .map(|i| PreCondition::ALL[i])
    }
}

impl fmt::Display for PreCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.attribute_name())
    }
}

/// A post-condition attribute, in the order of `enum class PostCond` and
/// `PostCondNames` (`include/behaviortree_cpp/tree_node.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PostCondition {
    /// `_onHalted`: runs after the node is halted.
    OnHalted,
    /// `_onFailure`: runs after the node returns FAILURE.
    OnFailure,
    /// `_onSuccess`: runs after the node returns SUCCESS.
    OnSuccess,
    /// `_post`: runs after each completion (SUCCESS, FAILURE or halt).
    Post,
}

impl PostCondition {
    /// All post-conditions, in the BT.CPP order.
    pub const ALL: [PostCondition; 4] = [
        PostCondition::OnHalted,
        PostCondition::OnFailure,
        PostCondition::OnSuccess,
        PostCondition::Post,
    ];

    /// The attribute name, for example `_onSuccess`.
    pub fn attribute_name(self) -> &'static str {
        POST_CONDITION_NAMES[self as usize]
    }

    /// The post-condition for an attribute name.
    pub fn from_attribute_name(name: &str) -> Option<PostCondition> {
        POST_CONDITION_NAMES
            .iter()
            .position(|&n| n == name)
            .map(|i| PostCondition::ALL[i])
    }
}

impl fmt::Display for PostCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.attribute_name())
    }
}

/// Runtime metadata attributes. Only the executor output
/// (`WriteTreeToXML(tree, true, ...)`) has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeMeta {
    /// `_uid`: the node UID.
    Uid,
    /// `_fullpath`: the path of a SubTree instance.
    FullPath,
}

/// The class of an attribute of a tree node element.
///
/// The classes follow `createNodeFromXML` and `recursivelyCreateSubtree`
/// (`src/xml_parsing.cpp`):
///
/// - An attribute is a port (or a SubTree remapping) if
///   `IsAllowedPortName` is true for its name.
/// - `name`, `ID`, `_autoremap`, the pre-conditions and the post-conditions
///   are reserved (`IsReservedAttribute`).
/// - BT.CPP keeps all other attributes in `NodeConfig::other_attributes`
///   and does not use them as ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttributeClass {
    /// `name`: the instance name.
    Name,
    /// `ID`: the type ID of the explicit form, or the tree ID of a
    /// `<SubTree>`.
    Id,
    /// `_autoremap` (SubTree only).
    AutoRemap,
    /// A pre-condition script.
    PreCondition(PreCondition),
    /// A post-condition script.
    PostCondition(PostCondition),
    /// `_uid` or `_fullpath`.
    RuntimeMeta(RuntimeMeta),
    /// Another name that starts with `_`. BT.CPP ignores it.
    OtherUnderscore,
    /// A port value or a SubTree remapping.
    Port,
    /// A name that is not reserved, does not start with `_`, and is not an
    /// allowed port name (for example `1x`, `a.b` or a name that starts with
    /// a non-ASCII letter). BT.CPP ignores it; it is not a port.
    NotAPort,
}

impl AttributeClass {
    /// Classifies an attribute name.
    pub fn of(name: &str) -> AttributeClass {
        match name {
            "name" => return AttributeClass::Name,
            "ID" => return AttributeClass::Id,
            "_autoremap" => return AttributeClass::AutoRemap,
            "_uid" => return AttributeClass::RuntimeMeta(RuntimeMeta::Uid),
            "_fullpath" => return AttributeClass::RuntimeMeta(RuntimeMeta::FullPath),
            _ => {}
        }
        if let Some(pre) = PreCondition::from_attribute_name(name) {
            AttributeClass::PreCondition(pre)
        } else if let Some(post) = PostCondition::from_attribute_name(name) {
            AttributeClass::PostCondition(post)
        } else if is_allowed_port_name(name) {
            AttributeClass::Port
        } else if name.starts_with('_') {
            AttributeClass::OtherUnderscore
        } else {
            AttributeClass::NotAPort
        }
    }

    /// True for [`AttributeClass::Port`].
    pub fn is_port(self) -> bool {
        self == AttributeClass::Port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_names() {
        for (i, pre) in PreCondition::ALL.iter().enumerate() {
            assert_eq!(pre.attribute_name(), PRE_CONDITION_NAMES[i]);
            assert_eq!(
                PreCondition::from_attribute_name(pre.attribute_name()),
                Some(*pre)
            );
        }
        for (i, post) in PostCondition::ALL.iter().enumerate() {
            assert_eq!(post.attribute_name(), POST_CONDITION_NAMES[i]);
            assert_eq!(
                PostCondition::from_attribute_name(post.attribute_name()),
                Some(*post)
            );
        }
        assert_eq!(PreCondition::SkipIf.to_string(), "_skipIf");
        assert_eq!(PostCondition::OnHalted.to_string(), "_onHalted");
        assert_eq!(PreCondition::from_attribute_name("_skipif"), None);
    }

    #[test]
    fn classes() {
        use AttributeClass as C;
        assert_eq!(C::of("name"), C::Name);
        assert_eq!(C::of("ID"), C::Id);
        assert_eq!(C::of("_autoremap"), C::AutoRemap);
        assert_eq!(C::of("_while"), C::PreCondition(PreCondition::While));
        assert_eq!(C::of("_post"), C::PostCondition(PostCondition::Post));
        assert_eq!(C::of("_uid"), C::RuntimeMeta(RuntimeMeta::Uid));
        assert_eq!(C::of("_fullpath"), C::RuntimeMeta(RuntimeMeta::FullPath));
        assert_eq!(C::of("_description"), C::OtherUnderscore);
        assert_eq!(C::of("goal"), C::Port);
        assert_eq!(C::of("id"), C::Port);
        assert_eq!(C::of("2goal"), C::NotAPort);
        assert_eq!(C::of("a.b"), C::NotAPort);
        assert_eq!(C::of("Ügoal"), C::NotAPort);
        assert!(C::of("x").is_port());
        assert!(!C::of("name").is_port());
    }
}
