//! Checks of attribute values: blackboard references, bare `&` characters
//! and the conversion of literal port values.

use crate::bt::names::parse_bool;

/// Why a value is not a correct blackboard reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RefProblem {
    /// The value starts with `{` but does not end with `}`.
    NoClosingBrace,
    /// The value ends with `}` but has no `{`.
    NoOpeningBrace,
    /// `{}` or `{@}`: the key is empty.
    EmptyKey,
    /// The key contains `{` or `}`.
    BraceInKey(String),
    /// The key starts or ends with white space.
    SpaceInKey(String),
}

/// Checks a port value (or a SubTree remapping) that looks like a blackboard
/// reference.
///
/// BT.CPP removes spaces (only U+0020) before `{` and after `}`
/// (`TreeNode::isBlackboardPointer` in `src/tree_node.cpp`). A value of
/// three or more characters between `{` and `}` is a reference; all other
/// values are text. These cases are probably mistakes:
///
/// - `{key` or `key}`: BT.CPP reads the text, not a reference.
/// - `{}`: text; `{@}`: a reference to the empty key of the root
///   blackboard.
/// - `{a}{b}` and `{"x":{"y":1}}`: a reference with the key `a}{b`. (A
///   JSON value needs the prefix `json:`.)
/// - `{ key }`: a reference with the key ` key `.
///
/// `{=}` is correct.
pub(super) fn check_blackboard_ref(value: &str) -> Option<RefProblem> {
    let trimmed = value.trim_matches(' ');
    let starts = trimmed.starts_with('{');
    let ends = trimmed.ends_with('}');
    if starts && !ends {
        return Some(RefProblem::NoClosingBrace);
    }
    if ends && !starts {
        // A text such as `json:{"x":1}` has an opening brace inside.
        return (!trimmed.contains('{')).then_some(RefProblem::NoOpeningBrace);
    }
    if !starts {
        return None;
    }
    if trimmed.len() < 3 {
        return Some(RefProblem::EmptyKey);
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if inner == "=" {
        return None;
    }
    let key = inner.strip_prefix('@').unwrap_or(inner);
    if key.is_empty() {
        return Some(RefProblem::EmptyKey);
    }
    if inner.contains(['{', '}']) {
        return Some(RefProblem::BraceInKey(inner.to_owned()));
    }
    if key.trim() != key || inner.trim() != inner {
        return Some(RefProblem::SpaceInKey(inner.to_owned()));
    }
    None
}

/// True if `createNodeFromXML` (`src/xml_parsing.cpp`) treats the value as
/// a blackboard reference before it tries to convert a literal. The test is
/// on the raw value: at least three bytes, `{` first and `}` last. (Spaces
/// around the braces are not removed here, unlike in
/// `isBlackboardPointer`.)
pub(super) fn is_blackboard_for_conversion(value: &str) -> bool {
    value.len() >= 3 && value.starts_with('{') && value.ends_with('}')
}

/// Finds the first `&` in a raw attribute value that does not start a
/// reference that tinyxml2 decodes: `&quot;`, `&amp;`, `&apos;`, `&lt;`,
/// `&gt;`, `&#NNN;` or `&#xHHH;` with a code point from 1 to U+10FFFF
/// (`StrPair::GetStr` and `XMLUtil::GetCharacterRef` in tinyxml2). A
/// reference to U+0000 also counts: it ends the value. Returns the byte
/// offset of the `&`.
pub(super) fn bad_ampersand(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while let Some(pos) = raw[i..].find('&') {
        let at = i + pos;
        match reference_len(&bytes[at..]) {
            Some(len) => i = at + len,
            None => return Some(at),
        }
    }
    None
}

/// The length of a valid reference at the start of `s` (which starts with
/// `&`), or `None`.
fn reference_len(s: &[u8]) -> Option<usize> {
    for name in ["quot", "amp", "apos", "lt", "gt"] {
        let n = name.len();
        if s.len() > n + 1 && &s[1..=n] == name.as_bytes() && s[n + 1] == b';' {
            return Some(n + 2);
        }
    }
    if s.get(1) != Some(&b'#') {
        return None;
    }
    // tinyxml2 reads a hexadecimal reference only after a lowercase `x`.
    let (hex, start) = if s.get(2) == Some(&b'x') {
        (true, 3)
    } else {
        (false, 2)
    };
    let semi = s.iter().skip(start).position(|&c| c == b';')? + start;
    let digits = &s[start..semi];
    if digits.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &d in digits {
        let digit = if hex {
            char::from(d).to_digit(16)?
        } else {
            char::from(d).to_digit(10)?
        };
        value = value
            .checked_mul(if hex { 16 } else { 10 })?
            .checked_add(digit)?;
        if value > 0x10_FFFF {
            return None;
        }
    }
    (value != 0).then_some(semi + 1)
}

/// Decodes a raw attribute value as a strict XML reader would: the valid
/// references of [`bad_ampersand`] are replaced, and a bare `&` stays. Line
/// ends become LF, as in tinyxml2. The validator compares the result with
/// the value that BT.CPP reads ([`crate::xml::Attribute::value`]): if they
/// differ, tinyxml2 changed the value.
pub(super) fn decode_strict(raw: &str) -> String {
    let normalized = raw
        .replace("\r\n", "\n")
        .replace("\n\r", "\n")
        .replace('\r', "\n");
    let bytes = normalized.as_bytes();
    let mut out = String::with_capacity(normalized.len());
    let mut i = 0;
    while let Some(pos) = normalized[i..].find('&') {
        let at = i + pos;
        out.push_str(&normalized[i..at]);
        match reference_len(&bytes[at..]) {
            Some(len) => {
                out.push(decode_reference(&normalized[at..at + len]));
                i = at + len;
            }
            None => {
                out.push('&');
                i = at + 1;
            }
        }
    }
    out.push_str(&normalized[i..]);
    out
}

/// Decodes one valid reference (see [`reference_len`]).
fn decode_reference(reference: &str) -> char {
    let body = &reference[1..reference.len() - 1];
    match body {
        "quot" => '"',
        "amp" => '&',
        "apos" => '\'',
        "lt" => '<',
        "gt" => '>',
        _ => {
            let code = match body.strip_prefix("#x") {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => body.strip_prefix('#').and_then(|d| d.parse().ok()),
            };
            code.and_then(char::from_u32)
                .unwrap_or(char::REPLACEMENT_CHARACTER)
        }
    }
}

/// Checks if BT.CPP can convert a literal port value to a port type.
///
/// Returns `None` if FastTick does not know the conversion of the type.
/// BT.CPP checks literal values only for strongly typed ports with a
/// converter (`createNodeFromXML`); `GetAnyFromStringFunctor`
/// (`include/behaviortree_cpp/basic_types.h`) gives no converter for
/// `BT::AnyTypeAllowed` and for enum types such as `BT::NodeStatus`. The
/// known types are the built-in specializations of `convertFromString` in
/// `src/basic_types.cpp`, with the type names that `BT::demangle` writes on
/// Linux:
///
/// - integers: `std::from_chars` on the start of the text (a `-` only for
///   signed types, no `+`, no leading spaces; the text after the number is
///   ignored), then a range check (`ConvertWithBoundCheck`);
/// - `double` and `float`: `std::from_chars` (also `inf` and `nan`); a
///   value out of range fails;
/// - `bool`: `convertFromString<bool>`;
/// - `std::string`: always valid.
pub(super) fn literal_converts(type_name: &str, value: &str) -> Option<bool> {
    let ok = match type_name {
        "std::string" => true,
        "bool" => parse_bool(value).is_some(),
        "signed char" => int_in_range(value, i64::from(i8::MIN), i64::from(i8::MAX)),
        "short" => int_in_range(value, i64::from(i16::MIN), i64::from(i16::MAX)),
        "int" => int_in_range(value, i64::from(i32::MIN), i64::from(i32::MAX)),
        "long" => int_in_range(value, i64::MIN, i64::MAX),
        "unsigned char" => int_in_range(value, 0, i64::from(u8::MAX)),
        "unsigned short" => int_in_range(value, 0, i64::from(u16::MAX)),
        "unsigned int" => int_in_range(value, 0, i64::from(u32::MAX)),
        "unsigned long" => unsigned_long(value),
        "double" => float_prefix(value, false),
        "float" => float_prefix(value, true),
        _ => return None,
    };
    Some(ok)
}

/// The decimal digits at the start of `s`, after an optional `-`.
/// Returns (negative, digits).
fn integer_prefix(s: &str) -> Option<(bool, &str)> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let end = rest
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| (negative, &rest[..end]))
}

/// `convertFromString<int64_t>` (from_chars) and then the range check of
/// `ConvertWithBoundCheck`.
fn int_in_range(s: &str, min: i64, max: i64) -> bool {
    let Some((negative, digits)) = integer_prefix(s) else {
        return false;
    };
    let magnitude: i128 = match digits.trim_start_matches('0') {
        "" => 0,
        d if d.len() > 20 => return false,
        d => match d.parse() {
            Ok(v) => v,
            Err(_) => return false,
        },
    };
    let value = if negative { -magnitude } else { magnitude };
    // from_chars into int64_t fails out of range.
    let Ok(value) = i64::try_from(value) else {
        return false;
    };
    (min..=max).contains(&value)
}

/// `convertFromString<uint64_t>`: from_chars for an unsigned type does not
/// accept a sign.
fn unsigned_long(s: &str) -> bool {
    match integer_prefix(s) {
        Some((false, digits)) => match digits.trim_start_matches('0') {
            "" => true,
            d => d.parse::<u64>().is_ok(),
        },
        _ => false,
    }
}

/// `std::from_chars` for `double` or `float` (format `general`): an
/// optional `-`, then `inf`, `infinity` or `nan` (any case), or a decimal
/// number with an optional exponent. The text after the number is ignored.
/// A number out of range (too large, or so small that it becomes zero)
/// fails.
fn float_prefix(s: &str, single: bool) -> bool {
    let rest = s.strip_prefix('-').unwrap_or(s);
    let lower = rest.get(..3).map(str::to_ascii_lowercase);
    if matches!(lower.as_deref(), Some("inf" | "nan")) {
        return true;
    }
    let b = rest.as_bytes();
    let mut i = 0;
    let mut nonzero = false;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        nonzero |= b[i] != b'0';
        digits += 1;
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            nonzero |= b[i] != b'0';
            digits += 1;
            i += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    let mut end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exponent_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exponent_start {
            end = j;
        }
    }
    let number = &rest[..end];
    let (infinite, zero) = if single {
        match number.parse::<f32>() {
            Ok(v) => (v.is_infinite(), v == 0.0),
            Err(_) => return false,
        }
    } else {
        match number.parse::<f64>() {
            Ok(v) => (v.is_infinite(), v == 0.0),
            Err(_) => return false,
        }
    };
    !infinite && !(zero && nonzero)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blackboard_refs() {
        assert_eq!(check_blackboard_ref("{key}"), None);
        assert_eq!(check_blackboard_ref(" {@key} "), None);
        assert_eq!(check_blackboard_ref("{=}"), None);
        assert_eq!(check_blackboard_ref("text"), None);
        assert_eq!(check_blackboard_ref(""), None);
        assert_eq!(check_blackboard_ref("json:{\"x\":1}"), None);
        assert_eq!(check_blackboard_ref("a{b}c"), None);
        assert_eq!(
            check_blackboard_ref("{key"),
            Some(RefProblem::NoClosingBrace)
        );
        assert_eq!(
            check_blackboard_ref("key}"),
            Some(RefProblem::NoOpeningBrace)
        );
        assert_eq!(check_blackboard_ref("{}"), Some(RefProblem::EmptyKey));
        assert_eq!(check_blackboard_ref(" {} "), Some(RefProblem::EmptyKey));
        assert_eq!(check_blackboard_ref("{@}"), Some(RefProblem::EmptyKey));
        assert_eq!(
            check_blackboard_ref("{a}{b}"),
            Some(RefProblem::BraceInKey("a}{b".into()))
        );
        assert_eq!(
            check_blackboard_ref("{ key }"),
            Some(RefProblem::SpaceInKey(" key ".into()))
        );
        assert_eq!(
            check_blackboard_ref("{@ key}"),
            Some(RefProblem::SpaceInKey("@ key".into()))
        );
        assert_eq!(
            check_blackboard_ref("{key\t}"),
            Some(RefProblem::SpaceInKey("key\t".into()))
        );
    }

    #[test]
    fn conversion_blackboard_test_is_raw() {
        assert!(is_blackboard_for_conversion("{x}"));
        assert!(!is_blackboard_for_conversion(" {x}"));
        assert!(!is_blackboard_for_conversion("{}"));
        assert!(!is_blackboard_for_conversion("="));
    }

    #[test]
    fn ampersands() {
        assert_eq!(bad_ampersand("a &amp; b &lt;&gt;&quot;&apos;"), None);
        assert_eq!(bad_ampersand("&#65;&#x41;&#x10FFFF;"), None);
        assert_eq!(bad_ampersand("a && b"), Some(2));
        assert_eq!(bad_ampersand("&amp;&"), Some(5));
        assert_eq!(bad_ampersand("&nbsp;"), Some(0));
        assert_eq!(bad_ampersand("&#X41;"), Some(0));
        assert_eq!(bad_ampersand("&#0;"), Some(0));
        assert_eq!(bad_ampersand("&#;"), Some(0));
        assert_eq!(bad_ampersand("&#x110000;"), Some(0));
        assert_eq!(bad_ampersand("&#12a;"), Some(0));
        assert_eq!(bad_ampersand("&#12"), Some(0));
        assert_eq!(bad_ampersand("&"), Some(0));
        assert_eq!(bad_ampersand("no ampersand"), None);
    }

    #[test]
    fn strict_decode() {
        assert_eq!(decode_strict("a &lt; b && c"), "a < b && c");
        assert_eq!(decode_strict("&#65;&#x42;&amp;"), "AB&");
        assert_eq!(decode_strict("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(decode_strict("&nbsp;"), "&nbsp;");
    }

    #[test]
    fn integer_conversion() {
        for (t, v, ok) in [
            ("int", "42", true),
            ("int", "-42", true),
            ("int", "12abc", true),
            ("int", "007", true),
            ("int", "abc", false),
            ("int", "", false),
            ("int", " 1", false),
            ("int", "+1", false),
            ("int", "-", false),
            ("int", "2147483647", true),
            ("int", "2147483648", false),
            ("int", "-2147483648", true),
            ("int", "99999999999", false),
            ("unsigned int", "4294967295", true),
            ("unsigned int", "4294967296", false),
            ("unsigned int", "-1", false),
            ("unsigned int", "-0", true),
            ("unsigned char", "255", true),
            ("unsigned char", "256", false),
            ("signed char", "-128", true),
            ("short", "40000", false),
            ("unsigned short", "65535", true),
            ("long", "9223372036854775807", true),
            ("long", "9223372036854775808", false),
            ("long", "-9223372036854775808", true),
            ("long", "000000000000000000000000001", true),
            ("unsigned long", "18446744073709551615", true),
            ("unsigned long", "18446744073709551616", false),
            ("unsigned long", "-1", false),
            ("unsigned long", "0", true),
        ] {
            assert_eq!(literal_converts(t, v), Some(ok), "{t} {v:?}");
        }
    }

    #[test]
    fn float_conversion() {
        for (t, v, ok) in [
            ("double", "1.5", true),
            ("double", "-1.5", true),
            ("double", ".5", true),
            ("double", "5.", true),
            ("double", "1e3", true),
            ("double", "1e", true),
            ("double", "1.5abc", true),
            ("double", "inf", true),
            ("double", "-Infinity", true),
            ("double", "NaN", true),
            ("double", "+1", false),
            ("double", ".", false),
            ("double", "abc", false),
            ("double", "", false),
            ("double", " 1", false),
            ("double", "1e999", false),
            ("double", "1e-999", false),
            ("double", "0.0", true),
            ("double", "0e999", true),
            ("float", "1e39", false),
            ("float", "1e38", true),
        ] {
            assert_eq!(literal_converts(t, v), Some(ok), "{t} {v:?}");
        }
    }

    #[test]
    fn other_types() {
        assert_eq!(literal_converts("bool", "true"), Some(true));
        assert_eq!(literal_converts("bool", "yes"), Some(false));
        assert_eq!(literal_converts("std::string", ""), Some(true));
        assert_eq!(literal_converts("BT::NodeStatus", "BAD"), None);
        assert_eq!(literal_converts("BT::AnyTypeAllowed", "x"), None);
        assert_eq!(literal_converts("Pose2D", "x"), None);
        assert_eq!(literal_converts("long long", "x"), None);
    }
}
