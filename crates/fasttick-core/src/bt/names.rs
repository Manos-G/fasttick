//! Name rules of BT.CPP 4.9.0.
//!
//! The functions in this module copy the rules of `src/basic_types.cpp`
//! (`IsAllowedPortName`, `IsReservedAttribute`, `findForbiddenChar`,
//! `convertFromString<bool>`) and of `src/xml_parsing.cpp`
//! (`validateModelName`, `validatePortName`, `validateInstanceName`).
//! BT.CPP checks bytes; these functions check the UTF-8 bytes of the name in
//! the same way.

use std::fmt;

/// The pre-condition attribute names, in the order of `PreCondNames`
/// (`include/behaviortree_cpp/tree_node.h`).
pub const PRE_CONDITION_NAMES: [&str; 4] = ["_failureIf", "_successIf", "_skipIf", "_while"];

/// The post-condition attribute names, in the order of `PostCondNames`
/// (`include/behaviortree_cpp/tree_node.h`).
pub const POST_CONDITION_NAMES: [&str; 4] = ["_onHalted", "_onFailure", "_onSuccess", "_post"];

/// The characters that `findForbiddenChar` rejects in model names and port
/// names (`kForbiddenChars` in `src/basic_types.cpp`). ASCII control
/// characters (0 to 31 and 127) are also forbidden.
pub const FORBIDDEN_CHARS: [char; 16] = [
    ' ', '\t', '\n', '\r', '<', '>', '&', '"', '\'', '/', '\\', ':', '*', '?', '|', '.',
];

/// Returns the first forbidden character of a model name or port name, or
/// `None` if the name has no forbidden character.
///
/// Copy of `findForbiddenChar` (`src/basic_types.cpp`): bytes with the high
/// bit set (parts of UTF-8 sequences) are allowed; ASCII control characters
/// and [`FORBIDDEN_CHARS`] are not.
pub fn find_forbidden_char(name: &str) -> Option<char> {
    name.chars()
        .find(|&c| c.is_ascii() && (c.is_ascii_control() || FORBIDDEN_CHARS.contains(&c)))
}

/// Returns true for the attribute names that BT.CPP reserves: `name`, `ID`,
/// `_autoremap`, the pre-conditions and the post-conditions.
///
/// Copy of `IsReservedAttribute` (`src/basic_types.cpp`).
pub fn is_reserved_attribute(name: &str) -> bool {
    PRE_CONDITION_NAMES.contains(&name)
        || POST_CONDITION_NAMES.contains(&name)
        || name == "name"
        || name == "ID"
        || name == "_autoremap"
}

/// Returns true if BT.CPP reads an attribute with this name as a port (or as
/// a SubTree remapping).
///
/// Copy of `IsAllowedPortName` (`src/basic_types.cpp`): the name is not
/// empty, the first byte is an ASCII letter (`std::isalpha` in the C locale),
/// the name has no forbidden character, and it is not reserved.
pub fn is_allowed_port_name(name: &str) -> bool {
    let Some(first) = name.as_bytes().first() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && find_forbidden_char(name).is_none()
        && !is_reserved_attribute(name)
}

/// Why a name is not valid. The messages follow the error messages of BT.CPP.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NameError {
    /// The name is empty.
    Empty,
    /// The name is `Root` or `root` (reserved for model names).
    ReservedRoot,
    /// A port name starts with a digit.
    StartsWithDigit,
    /// The name contains a forbidden character.
    ForbiddenChar(char),
    /// A port name is a reserved attribute name.
    ReservedAttribute,
    /// An instance name contains a control character that XML cannot hold.
    ControlChar(char),
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Empty => f.write_str("the name is empty"),
            NameError::ReservedRoot => {
                f.write_str("'Root' is a reserved name and cannot be used as a node type")
            }
            NameError::StartsWithDigit => f.write_str("a port name cannot start with a digit"),
            NameError::ForbiddenChar(c) => {
                write!(
                    f,
                    "the name contains the forbidden character {}",
                    describe_char(*c)
                )
            }
            NameError::ReservedAttribute => f.write_str("the name is a reserved attribute name"),
            NameError::ControlChar(c) => write!(
                f,
                "the name contains the control character {}",
                describe_char(*c)
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Describes a character for an error message, like `formatForbiddenChar`
/// in `src/xml_parsing.cpp`.
fn describe_char(c: char) -> String {
    if c.is_ascii_control() {
        format!("control character (ASCII {})", c as u32)
    } else {
        format!("'{c}'")
    }
}

/// Checks a model name (a node type ID or a `<BehaviorTree>` ID).
///
/// Copy of `validateModelName` (`src/xml_parsing.cpp`).
pub fn validate_model_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name == "Root" || name == "root" {
        return Err(NameError::ReservedRoot);
    }
    match find_forbidden_char(name) {
        Some(c) => Err(NameError::ForbiddenChar(c)),
        None => Ok(()),
    }
}

/// Checks a port name in a model.
///
/// Copy of `validatePortName` (`src/xml_parsing.cpp`). This rule is less
/// strict than [`is_allowed_port_name`]: a name that starts with `_` or with
/// a non-ASCII character passes here, but BT.CPP does not read an attribute
/// with that name as a port.
pub fn validate_port_name(name: &str) -> Result<(), NameError> {
    let Some(first) = name.as_bytes().first() else {
        return Err(NameError::Empty);
    };
    if first.is_ascii_digit() {
        return Err(NameError::StartsWithDigit);
    }
    if let Some(c) = find_forbidden_char(name) {
        return Err(NameError::ForbiddenChar(c));
    }
    if is_reserved_attribute(name) {
        return Err(NameError::ReservedAttribute);
    }
    Ok(())
}

/// Checks an instance name (the `name` attribute).
///
/// Copy of `validateInstanceName` (`src/xml_parsing.cpp`): an empty name is
/// valid; ASCII control characters other than tab, LF and CR are not, and
/// neither is DEL (127).
pub fn validate_instance_name(name: &str) -> Result<(), NameError> {
    match name
        .chars()
        .find(|&c| (c < ' ' && !matches!(c, '\t' | '\n' | '\r')) || c == '\u{7f}')
    {
        Some(c) => Err(NameError::ControlChar(c)),
        None => Ok(()),
    }
}

/// Converts a text to a boolean like `convertFromString<bool>`
/// (`src/basic_types.cpp`). It accepts `0`, `1`, `true`, `TRUE`, `True`,
/// `false`, `FALSE` and `False`. Other texts give `None` (BT.CPP throws).
pub fn parse_bool(text: &str) -> Option<bool> {
    match text {
        "1" | "true" | "TRUE" | "True" => Some(true),
        "0" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_chars() {
        assert_eq!(find_forbidden_char("MyAction"), None);
        assert_eq!(find_forbidden_char("My-Action_2"), None);
        assert_eq!(find_forbidden_char("检查门状态"), None);
        assert_eq!(find_forbidden_char("Tür_öffnen"), None);
        assert_eq!(find_forbidden_char("My Action"), Some(' '));
        assert_eq!(find_forbidden_char("request.name"), Some('.'));
        assert_eq!(find_forbidden_char("a/b"), Some('/'));
        assert_eq!(find_forbidden_char("a\u{7f}"), Some('\u{7f}'));
        assert_eq!(find_forbidden_char("a\u{1}"), Some('\u{1}'));
        for c in FORBIDDEN_CHARS {
            assert_eq!(find_forbidden_char(&format!("x{c}y")), Some(c));
        }
    }

    #[test]
    fn reserved_attributes() {
        for name in PRE_CONDITION_NAMES.iter().chain(&POST_CONDITION_NAMES) {
            assert!(is_reserved_attribute(name));
        }
        assert!(is_reserved_attribute("name"));
        assert!(is_reserved_attribute("ID"));
        assert!(is_reserved_attribute("_autoremap"));
        assert!(!is_reserved_attribute("_uid"));
        assert!(!is_reserved_attribute("id"));
        assert!(!is_reserved_attribute("_description"));
    }

    #[test]
    fn allowed_port_names() {
        assert!(is_allowed_port_name("goal"));
        assert!(is_allowed_port_name("Goal_2"));
        assert!(is_allowed_port_name("a-b"));
        assert!(is_allowed_port_name("aÜ"));
        assert!(!is_allowed_port_name(""));
        assert!(!is_allowed_port_name("_goal"));
        assert!(!is_allowed_port_name("2goal"));
        assert!(!is_allowed_port_name("Ügoal"));
        assert!(!is_allowed_port_name("a.b"));
        assert!(!is_allowed_port_name("name"));
        assert!(!is_allowed_port_name("ID"));
    }

    #[test]
    fn model_names() {
        assert_eq!(validate_model_name("MainTree"), Ok(()));
        assert_eq!(validate_model_name(""), Err(NameError::Empty));
        assert_eq!(validate_model_name("Root"), Err(NameError::ReservedRoot));
        assert_eq!(validate_model_name("root"), Err(NameError::ReservedRoot));
        assert_eq!(validate_model_name("ROOT"), Ok(()));
        assert_eq!(
            validate_model_name("a:b"),
            Err(NameError::ForbiddenChar(':'))
        );
    }

    #[test]
    fn port_names() {
        assert_eq!(validate_port_name("goal"), Ok(()));
        assert_eq!(validate_port_name("_private"), Ok(()));
        assert_eq!(validate_port_name(""), Err(NameError::Empty));
        assert_eq!(validate_port_name("1x"), Err(NameError::StartsWithDigit));
        assert_eq!(
            validate_port_name("a b"),
            Err(NameError::ForbiddenChar(' '))
        );
        assert_eq!(
            validate_port_name("_skipIf"),
            Err(NameError::ReservedAttribute)
        );
        assert_eq!(
            validate_port_name("name"),
            Err(NameError::ReservedAttribute)
        );
    }

    #[test]
    fn instance_names() {
        assert_eq!(validate_instance_name(""), Ok(()));
        assert_eq!(validate_instance_name("My Action. 1"), Ok(()));
        assert_eq!(validate_instance_name("a\tb\nc\rd"), Ok(()));
        assert_eq!(
            validate_instance_name("bell\u{7}"),
            Err(NameError::ControlChar('\u{7}'))
        );
        assert_eq!(
            validate_instance_name("del\u{7f}"),
            Err(NameError::ControlChar('\u{7f}'))
        );
    }

    #[test]
    fn bools() {
        for t in ["1", "true", "TRUE", "True"] {
            assert_eq!(parse_bool(t), Some(true));
        }
        for t in ["0", "false", "FALSE", "False"] {
            assert_eq!(parse_bool(t), Some(false));
        }
        for t in ["", "yes", "tRUE", " true", "2"] {
            assert_eq!(parse_bool(t), None);
        }
    }

    #[test]
    fn error_messages() {
        assert!(
            NameError::ForbiddenChar('\u{1}')
                .to_string()
                .contains("ASCII 1")
        );
        assert!(NameError::ForbiddenChar('.').to_string().contains("'.'"));
    }
}
