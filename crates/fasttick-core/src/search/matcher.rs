//! Text matching for [`super::Query`].

use std::ops::Range;

/// A compiled query text.
#[derive(Debug, Clone)]
pub(super) struct Matcher {
    /// The pattern, folded when the match ignores case.
    pattern: Vec<char>,
    /// The same pattern as bytes, if it is all ASCII (for the fast path).
    ascii: Option<Vec<u8>>,
    glob: bool,
    case_sensitive: bool,
    whole_word: bool,
}

impl Matcher {
    /// Compiles a pattern. `wildcard` turns on glob matching, but only if
    /// the pattern contains `*` or `?`.
    pub(super) fn new(
        text: &str,
        case_sensitive: bool,
        whole_word: bool,
        wildcard: bool,
    ) -> Matcher {
        let glob = wildcard && text.chars().any(|c| c == '*' || c == '?');
        let pattern: Vec<char> = text.chars().map(|c| fold(c, case_sensitive)).collect();
        let ascii = pattern
            .iter()
            .all(char::is_ascii)
            .then(|| pattern.iter().map(|&c| c as u8).collect());
        Matcher {
            pattern,
            ascii,
            glob,
            case_sensitive,
            whole_word,
        }
    }

    /// Finds the first match in `text` and returns its byte range.
    ///
    /// - Glob mode: the pattern must match the whole text. `*` matches any
    ///   sequence of characters (also an empty one) and `?` matches one
    ///   character. The range is the whole text.
    /// - Else: the first occurrence of the pattern as a substring. With
    ///   `whole_word`, the characters before and after the occurrence must
    ///   not be word characters (letters, digits or `_`).
    pub(super) fn find(&self, text: &str) -> Option<Range<usize>> {
        if self.pattern.is_empty() {
            return None;
        }
        if self.glob {
            let chars: Vec<char> = text.chars().map(|c| fold(c, self.case_sensitive)).collect();
            return glob_match(&self.pattern, &chars).then_some(0..text.len());
        }
        if let Some(pattern) = &self.ascii
            && text.is_ascii()
        {
            return self.find_ascii(pattern, text.as_bytes());
        }
        // Each text character with its byte offset; one extra entry for the
        // end of the text.
        let mut offsets: Vec<usize> = Vec::with_capacity(text.len() + 1);
        let mut chars: Vec<char> = Vec::with_capacity(text.len());
        for (offset, c) in text.char_indices() {
            offsets.push(offset);
            chars.push(fold(c, self.case_sensitive));
        }
        offsets.push(text.len());
        let m = self.pattern.len();
        if m > chars.len() {
            return None;
        }
        for start in 0..=chars.len() - m {
            if chars[start..start + m] != self.pattern[..] {
                continue;
            }
            if self.whole_word {
                let before = start.checked_sub(1).map(|i| chars[i]);
                let after = chars.get(start + m).copied();
                if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                    continue;
                }
            }
            return Some(offsets[start]..offsets[start + m]);
        }
        None
    }

    /// The substring search for an ASCII pattern in an ASCII text, without
    /// allocation. It gives the same result as the general search: for
    /// ASCII characters, `fold` is `to_ascii_lowercase`, and a byte is a
    /// character.
    fn find_ascii(&self, pattern: &[u8], text: &[u8]) -> Option<Range<usize>> {
        let m = pattern.len();
        if m > text.len() {
            return None;
        }
        let same = |a: u8, b: u8| {
            if self.case_sensitive {
                a == b
            } else {
                a.to_ascii_lowercase() == b
            }
        };
        let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        for start in 0..=text.len() - m {
            if !text[start..start + m]
                .iter()
                .zip(pattern)
                .all(|(&a, &b)| same(a, b))
            {
                continue;
            }
            if self.whole_word {
                let before = start.checked_sub(1).map(|i| text[i]);
                let after = text.get(start + m).copied();
                if before.is_some_and(word) || after.is_some_and(word) {
                    continue;
                }
            }
            return Some(start..start + m);
        }
        None
    }
}

/// Folds a character for a match that ignores case. The fold keeps one
/// character for one character, so byte ranges stay valid in the original
/// text: a character whose lower-case form has more than one character
/// stays as it is.
fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        return c;
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whole-text glob match with `*` and `?`. Linear in practice: on a
/// mismatch it goes back only to the last `*`.
fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut star_text = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            star_text = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            star_text += 1;
            t = star_text;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, text: &str, case: bool, word: bool, wild: bool) -> Option<Range<usize>> {
        Matcher::new(pattern, case, word, wild).find(text)
    }

    #[test]
    fn substring_ignores_case_by_default() {
        assert_eq!(find("base", "MoveBase", false, false, false), Some(4..8));
        assert_eq!(find("base", "MoveBase", true, false, false), None);
        assert_eq!(find("Base", "MoveBase", true, false, false), Some(4..8));
        assert_eq!(find("", "MoveBase", false, false, false), None);
        assert_eq!(find("MoveBaseX", "MoveBase", false, false, false), None);
    }

    #[test]
    fn ranges_are_byte_ranges() {
        // "é" is two bytes in UTF-8.
        assert_eq!(find("goal", "é goal", false, false, false), Some(3..7));
        assert_eq!(find("É", "café", false, false, false), Some(3..5));
    }

    #[test]
    fn whole_word() {
        assert_eq!(find("goal", "goal_pose", false, true, false), None);
        assert_eq!(find("goal", "{goal}", false, true, false), Some(1..5));
        assert_eq!(find("goal", "my goal is", false, true, false), Some(3..7));
        // The first occurrence is inside a word, the second is a word.
        assert_eq!(find("ab", "abc ab", false, true, false), Some(4..6));
    }

    #[test]
    fn wildcards_match_the_whole_text() {
        assert_eq!(find("Move*", "MoveBase", false, false, true), Some(0..8));
        assert_eq!(find("*base", "MoveBase", false, false, true), Some(0..8));
        assert_eq!(find("M?ve*", "MoveBase", false, false, true), Some(0..8));
        assert_eq!(find("Move?", "MoveBase", false, false, true), None);
        assert_eq!(find("*", "", false, false, true), Some(0..0));
        assert_eq!(find("a*b*c", "aXXbYYc", true, false, true), Some(0..7));
        assert_eq!(find("a*b*c", "aXXbYY", true, false, true), None);
        // Without the wildcard flag, '*' is a normal character.
        assert_eq!(find("a*", "xa*y", false, false, false), Some(1..3));
        // With the flag but without wildcard characters: substring.
        assert_eq!(find("ove", "MoveBase", false, false, true), Some(1..4));
    }

    #[test]
    fn ascii_fast_path_agrees_with_the_general_path() {
        let texts = [
            "MoveBase",
            "go_home",
            "{home_pose}",
            "a_b c",
            "",
            "ABCabc",
            "x == 10",
        ];
        let patterns = ["base", "Base", "home", "_", "b c", "abc", "=", "x"];
        for text in texts {
            for pattern in patterns {
                for (case, word) in [(false, false), (true, false), (false, true), (true, true)] {
                    let matcher = Matcher::new(pattern, case, word, false);
                    let fast = matcher.find(text);
                    // Force the general path: a non-ASCII suffix keeps the
                    // offsets of the ASCII part.
                    let general_text = format!("{text}\u{e9}");
                    let general = matcher.find(&general_text).filter(|r| r.end <= text.len());
                    // A match that needs the extra character is not a
                    // match in `text`; skip that rare case.
                    if general.is_some() || fast.is_none() {
                        assert_eq!(
                            fast, general,
                            "{pattern:?} in {text:?} case {case} word {word}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn glob_is_not_exponential() {
        let text: Vec<char> = "a".repeat(5_000).chars().collect();
        let pattern: Vec<char> = "*a*a*a*a*a*a*b".chars().collect();
        assert!(!glob_match(&pattern, &text));
    }
}
