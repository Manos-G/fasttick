//! Decoding and escaping of character data, and XML name rules.
//!
//! BT.CPP reads XML with tinyxml2 (`3rdparty/tinyxml2/tinyxml2.cpp`, version
//! 11.0.0), with the default settings: entity processing on and
//! `PRESERVE_WHITESPACE`. The functions in this module decode text and
//! attribute values exactly as tinyxml2 does, so that FastTick sees the same
//! values as BT.CPP.

use std::borrow::Cow;

/// The highest Unicode code point (`MAX_CODE_POINT` in
/// `XMLUtil::GetCharacterRef`).
const MAX_CODE_POINT: u32 = 0x10_FFFF;

/// The predefined entities (`entities[]` in `tinyxml2.cpp`), in the order
/// that tinyxml2 tries them.
const ENTITIES: [(&str, char); 5] = [
    ("quot", '"'),
    ("amp", '&'),
    ("apos", '\''),
    ("lt", '<'),
    ("gt", '>'),
];

/// The result of an attempt to decode one reference that starts with `&`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// The reference gives this character and uses `len` bytes.
    Char(char, usize),
    /// The reference gives the character U+0000. tinyxml2 writes a NUL byte,
    /// and the C string that it returns then ends at this point.
    Nul,
    /// tinyxml2 removes the `&` and keeps the next bytes as text. This
    /// happens only for the text `&#` at the end of the value.
    DropAmpersand,
    /// `&#` does not start a valid character reference. tinyxml2 copies the
    /// `&` and continues after it.
    Literal,
    /// `&` does not start a predefined entity. tinyxml2 moves its read and
    /// write positions by one byte but does not copy the byte (see
    /// [`decode`]).
    Unknown,
}

/// Decodes raw character data with the rules of tinyxml2
/// (`StrPair::GetStr` in `tinyxml2.cpp`).
///
/// - Line ends: CR LF becomes LF, a lone CR becomes LF, and LF CR becomes LF
///   (`NEEDS_NEWLINE_NORMALIZATION`). Tabs and other line ends stay as they
///   are. Only literal characters change; `&#13;` gives a CR.
/// - When `entities` is true (`NEEDS_ENTITY_PROCESSING`): the five predefined
///   entities and decimal or hexadecimal character references are resolved.
///   Other `&` characters stay literal.
///
/// Text, attribute values use `entities = true`. CDATA, comments and
/// processing instructions use `entities = false`.
///
/// tinyxml2 decodes in place, with a read position and a write position.
/// For an `&` that does not start a predefined entity or `&#`, it moves both
/// positions by one byte but does not copy the byte. While the two positions
/// are equal, the `&` stays. After an earlier reference or line end made the
/// text shorter, the byte at the write position is the source byte at that
/// index, so BT.CPP sees a wrong character there. For example, the value
/// `a &lt; b && c` becomes `a < b  b c`. This function gives the same
/// result. If that result is not valid UTF-8, the invalid bytes become
/// U+FFFD.
pub(crate) fn decode(raw: &str, entities: bool) -> Cow<'_, str> {
    let bytes = raw.as_bytes();
    let needs_work = bytes.iter().any(|&b| b == b'\r' || (entities && b == b'&'));
    if !needs_work {
        return Cow::Borrowed(raw);
    }
    // The bytes that tinyxml2 writes. `out.len()` is its write position and
    // `i` is its read position; `out.len() <= i` is always true.
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut copy_from = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                out.extend_from_slice(&bytes[copy_from..i]);
                out.push(b'\n');
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                copy_from = i;
            }
            b'\n' => {
                if bytes.get(i + 1) == Some(&b'\r') {
                    out.extend_from_slice(&bytes[copy_from..i]);
                    out.push(b'\n');
                    i += 2;
                    copy_from = i;
                } else {
                    i += 1;
                }
            }
            b'&' if entities => match decode_reference(&raw[i..]) {
                Reference::Char(c, len) => {
                    out.extend_from_slice(&bytes[copy_from..i]);
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    i += len;
                    copy_from = i;
                }
                Reference::Nul => {
                    out.extend_from_slice(&bytes[copy_from..i]);
                    return Cow::Owned(into_string(out));
                }
                Reference::DropAmpersand => {
                    out.extend_from_slice(&bytes[copy_from..i]);
                    i += 1;
                    copy_from = i;
                }
                Reference::Literal => i += 1,
                Reference::Unknown => {
                    out.extend_from_slice(&bytes[copy_from..i]);
                    // `++p; ++q;` without a copy: the byte at the write
                    // position is the source byte at that index.
                    let stale = bytes.get(out.len()).copied().unwrap_or(b'&');
                    out.push(stale);
                    i += 1;
                    copy_from = i;
                }
            },
            _ => i += 1,
        }
    }
    out.extend_from_slice(&bytes[copy_from..]);
    Cow::Owned(into_string(out))
}

/// Converts decoded bytes to a string. Invalid UTF-8 (possible only after
/// the stale-byte case of [`decode`]) becomes U+FFFD.
fn into_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Decodes one reference. `s` starts with `&`.
///
/// This follows `StrPair::GetStr` and `XMLUtil::GetCharacterRef` in
/// `tinyxml2.cpp`, including their unusual cases:
///
/// - The digits are read from the `;` backwards until the `#` (decimal) or
///   the `x` (hexadecimal) is found. Only a lowercase `x` starts a
///   hexadecimal reference.
/// - `&#;` and `&#x;` give the code point 0.
/// - A value above U+10FFFF is not a reference.
/// - The arithmetic wraps at 32 bits, as in C.
fn decode_reference(s: &str) -> Reference {
    let b = s.as_bytes();
    if b.get(1) == Some(&b'#') {
        if b.len() < 3 {
            // `*(p + 2)` is the terminating NUL: GetCharacterRef returns
            // `p + 1` with length 0, so the `&` disappears.
            return Reference::DropAmpersand;
        }
        let (hex, digits_start, terminator) = if b[2] == b'x' {
            (true, 3, b'x')
        } else {
            (false, 2, b'#')
        };
        if digits_start >= b.len() {
            return Reference::Literal;
        }
        let Some(semi) = b[digits_start..]
            .iter()
            .position(|&c| c == b';')
            .map(|p| p + digits_start)
        else {
            return Reference::Literal;
        };
        let radix: u32 = if hex { 16 } else { 10 };
        let mut ucs: u32 = 0;
        let mut mult: u32 = 1;
        let mut q = semi - 1;
        while b[q] != terminator {
            let c = b[q];
            let digit = match c {
                b'0'..=b'9' => u32::from(c - b'0'),
                b'a'..=b'f' if hex => u32::from(c - b'a') + 10,
                b'A'..=b'F' if hex => u32::from(c - b'A') + 10,
                _ => return Reference::Literal,
            };
            ucs = ucs.wrapping_add(mult.wrapping_mul(digit));
            mult = mult.wrapping_mul(radix);
            if mult > MAX_CODE_POINT {
                mult = MAX_CODE_POINT;
            }
            // `q` cannot pass the terminator at index 1 or 2.
            q -= 1;
        }
        if ucs > MAX_CODE_POINT {
            return Reference::Literal;
        }
        let len = semi + 1;
        if ucs == 0 {
            return Reference::Nul;
        }
        // tinyxml2 writes surrogate code points as (invalid) 3-byte UTF-8.
        // A Rust string cannot hold them, so we use U+FFFD.
        let c = char::from_u32(ucs).unwrap_or(char::REPLACEMENT_CHARACTER);
        return Reference::Char(c, len);
    }
    for (pattern, value) in ENTITIES {
        let n = pattern.len();
        if b.len() > n + 1 && &b[1..=n] == pattern.as_bytes() && b[n + 1] == b';' {
            return Reference::Char(value, n + 2);
        }
    }
    Reference::Unknown
}

/// Escapes an attribute value for the given quote character.
///
/// The result uses `&amp;`, `&lt;`, `&gt;`, the entity for the active quote
/// (`&quot;` or `&apos;`), `&#10;` for LF, `&#13;` for CR and `&#9;` for
/// TAB. tinyxml2 keeps literal line ends and tabs in attribute values, but
/// other XML tools change them to spaces, so FastTick writes references.
/// The other quote character stays literal.
pub(crate) fn escape_attribute(value: &str, quote: char) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote == '"' => out.push_str("&quot;"),
            '\'' if quote == '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes text content.
///
/// The result uses `&amp;`, `&lt;`, `&gt;` and `&#13;` for CR. Each LF
/// becomes `newline` (LF or CR LF, the line end of the document); tinyxml2
/// reads both as LF.
pub(crate) fn escape_text(text: &str, newline: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str(newline),
            c => out.push(c),
        }
    }
    out
}

/// Returns true if `c` can start a name (`XMLUtil::IsNameStartChar`):
/// an ASCII letter, `:`, `_` or any non-ASCII character.
pub(crate) fn is_name_start_char(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_alphabetic() || c == ':' || c == '_'
}

/// Returns true if `c` can be in a name (`XMLUtil::IsNameChar`): a name
/// start character, an ASCII digit, `.` or `-`.
pub(crate) fn is_name_char(c: char) -> bool {
    is_name_start_char(c) || c.is_ascii_digit() || c == '.' || c == '-'
}

/// Returns true if `name` is a valid element or attribute name for
/// tinyxml2.
///
/// The rules are those of `XMLUtil::IsNameStartChar` and
/// `XMLUtil::IsNameChar` in `tinyxml2.h`. They are less strict than the
/// XML 1.0 rules for non-ASCII characters.
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_name_start_char(c) => chars.all(is_name_char),
        _ => false,
    }
}

/// Returns the length in bytes of the name at the start of `s`, or 0 if
/// `s` does not start with a name.
pub(crate) fn name_len(s: &str) -> usize {
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, c)) if is_name_start_char(c) => {}
        _ => return 0,
    }
    for (i, c) in chars {
        if !is_name_char(c) {
            return i;
        }
    }
    s.len()
}

/// Returns true for the white-space characters of tinyxml2.
///
/// `XMLUtil::IsWhiteSpace` in `tinyxml2.h` uses `isspace` of the C locale:
/// space, tab, LF, CR, vertical tab (U+000B) and form feed (U+000C). This
/// is more than the four white-space characters of XML 1.0. tinyxml2 uses
/// the same set in tags, between top-level items and to find text with
/// only white space.
pub(crate) fn is_xml_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

/// Returns true if all characters of `s` are white space (see
/// [`is_xml_whitespace`]).
pub(crate) fn is_all_whitespace(s: &str) -> bool {
    s.bytes().all(is_xml_whitespace)
}

/// Returns `s` without the white space (see [`is_xml_whitespace`]) at its
/// start.
pub(crate) fn trim_whitespace_start(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii() && is_xml_whitespace(c as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(raw: &str) -> String {
        decode(raw, true).into_owned()
    }

    #[test]
    fn predefined_entities() {
        assert_eq!(d("&lt;&gt;&amp;&quot;&apos;"), "<>&\"'");
        assert_eq!(d("a &amp;&amp; b"), "a && b");
    }

    #[test]
    fn unknown_entities_and_lone_ampersands_stay() {
        assert_eq!(d("&foo; & &amp"), "&foo; & &amp");
        assert_eq!(d("&"), "&");
        assert_eq!(d("&AMP;"), "&AMP;");
    }

    #[test]
    fn character_references() {
        assert_eq!(d("&#65;&#x42;&#x63;"), "ABc");
        assert_eq!(d("&#x4e2d;&#20013;"), "中中");
        assert_eq!(d("&#10;&#9;&#13;"), "\n\t\r");
        assert_eq!(d("&#x1F600;"), "\u{1F600}");
        // Uppercase X is not a hexadecimal marker for tinyxml2.
        assert_eq!(d("&#X41;"), "&#X41;");
        // Bad digits: the `&` stays.
        assert_eq!(d("&#12a;"), "&#12a;");
        assert_eq!(d("&#xG;"), "&#xG;");
        // Out of range.
        assert_eq!(d("&#x110000;"), "&#x110000;");
        assert_eq!(d("&#99999999;"), "&#99999999;");
        // Leading zeros are allowed.
        assert_eq!(d("&#0000000065;"), "A");
        // No semicolon.
        assert_eq!(d("&#65"), "&#65");
    }

    #[test]
    fn tinyxml2_reference_quirks() {
        // The scan goes back from `;` to the first terminator.
        assert_eq!(d("&#x1x41;"), "A");
        assert_eq!(d("&#1#65;"), "A");
        // `&#` at the end: tinyxml2 drops the `&`.
        assert_eq!(d("a&#"), "a#");
        // Code point 0 ends the C string.
        assert_eq!(d("ab&#0;cd"), "ab");
        assert_eq!(d("ab&#;cd"), "ab");
        assert_eq!(d("ab&#x;cd"), "ab");
        // Surrogates cannot be in a Rust string.
        assert_eq!(d("&#xD800;"), "\u{FFFD}");
    }

    #[test]
    fn unknown_entity_after_a_shorter_reference_gets_a_stale_byte() {
        // The expected values are the output of tinyxml2 11.0.0 (the copy in
        // BT.CPP 4.9.0) for the same attribute values.
        assert_eq!(d("a &lt; b && c"), "a < b  b c");
        assert_eq!(d("&lt;&foo;"), "<lfoo;");
        assert_eq!(d("x&#65;&"), "xA#");
        // A line end that becomes shorter has the same effect.
        assert_eq!(d("&\r\n&#&foo;"), "&\n&##foo;");
        // Before the first shorter reference, the `&` stays.
        assert_eq!(d("a && b &lt;"), "a && b <");
        // The stale byte can be part of a multi-byte character: tinyxml2
        // gives the bytes 3C C3 A4 C3 A4 A4 78, which are not valid UTF-8.
        assert_eq!(d("&lt;\u{e4}\u{e4}&x"), "<\u{e4}\u{e4}\u{fffd}x");
        // Without entity processing, nothing changes.
        assert_eq!(decode("&lt;&foo;", false), "&lt;&foo;");
    }

    #[test]
    fn whitespace_set_of_tinyxml2() {
        for b in [b' ', b'\t', b'\n', b'\r', 0x0B, 0x0C] {
            assert!(is_xml_whitespace(b), "{b:#x}");
        }
        for b in [0u8, b'a', 0x85, 0xA0, 0x1F] {
            assert!(!is_xml_whitespace(b), "{b:#x}");
        }
        assert_eq!(trim_whitespace_start("\u{b}\u{c} \u{feff}x"), "\u{feff}x");
        assert!(is_all_whitespace(""));
        assert!(!is_all_whitespace("\u{a0}"));
    }

    #[test]
    fn newline_normalization() {
        assert_eq!(d("a\r\nb"), "a\nb");
        assert_eq!(d("a\rb"), "a\nb");
        assert_eq!(d("a\n\rb"), "a\nb");
        assert_eq!(d("a\r\n\rb"), "a\n\nb");
        assert_eq!(d("a\n\r\nb"), "a\n\nb");
        assert_eq!(d("a\n\tb"), "a\n\tb");
        assert_eq!(decode("x\r\n&amp;", false), "x\n&amp;");
    }

    #[test]
    fn borrowed_when_nothing_to_do() {
        assert!(matches!(decode("plain text\n", true), Cow::Borrowed(_)));
        assert!(matches!(decode("a & b", false), Cow::Borrowed(_)));
    }

    #[test]
    fn escape_attribute_round_trip() {
        let v = "a<b>&\"c'\n\t\r";
        let e = escape_attribute(v, '"');
        assert_eq!(e, "a&lt;b&gt;&amp;&quot;c'&#10;&#9;&#13;");
        assert_eq!(d(&e), v);
        let e = escape_attribute(v, '\'');
        assert_eq!(e, "a&lt;b&gt;&amp;\"c&apos;&#10;&#9;&#13;");
        assert_eq!(d(&e), v);
    }

    #[test]
    fn escape_text_round_trip() {
        let v = "a<b>&c\nd\re";
        assert_eq!(escape_text(v, "\n"), "a&lt;b&gt;&amp;c\nd&#13;e");
        assert_eq!(escape_text(v, "\r\n"), "a&lt;b&gt;&amp;c\r\nd&#13;e");
        assert_eq!(d(&escape_text(v, "\r\n")), v);
    }

    #[test]
    fn names() {
        assert!(is_valid_name("Sequence"));
        assert!(is_valid_name("_a:b.c-d1"));
        assert!(is_valid_name("ñame"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("1a"));
        assert!(!is_valid_name("-a"));
        assert!(!is_valid_name("a b"));
        assert!(!is_valid_name("a=b"));
        assert_eq!(name_len("abc def"), 3);
        assert_eq!(name_len("abc"), 3);
        assert_eq!(name_len("1bc"), 0);
        assert_eq!(name_len("ä/>"), 2);
    }
}
