//! Fuzzy matching for the search of node types.
//!
//! The characters of the query must be in the candidate in the same order,
//! but not side by side ("rsq" finds `ReactiveSequence`). Case does not
//! matter. The score is higher when the matched characters are side by
//! side, start words (`Reactive|Sequence`, `my_|node`), or start the
//! candidate, and when the candidate is short.

/// Points for each matched character.
const MATCH: i32 = 16;
/// Extra points when a matched character follows the previous match.
const CONSECUTIVE: i32 = 14;
/// Extra points when a matched character starts a word.
const WORD_START: i32 = 12;
/// Extra points when the first character of the query is the first
/// character of the candidate.
const FIRST: i32 = 10;
/// Points lost for each character between two matches (at most
/// [`MAX_GAP_PENALTY`] for one gap).
const GAP: i32 = 2;
const MAX_GAP_PENALTY: i32 = 12;
/// Extra points when the candidate starts with the query.
const PREFIX: i32 = 30;
/// Extra points when the candidate is the query.
const EXACT: i32 = 100;

/// The score of `candidate` for `query`, or `None` if the candidate does
/// not have the characters of the query in order. White space in the query
/// is ignored; an empty query matches all candidates with score 0. Higher
/// is better.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let (m, n) = (query.len(), chars.len());
    if m > n {
        return None;
    }
    // best[j]: the best score of the query so far, with its last character
    // matched at position j of the candidate.
    let mut best: Vec<Option<i32>> = vec![None; n];
    for (i, q) in query.iter().enumerate() {
        let mut next: Vec<Option<i32>> = vec![None; n];
        for j in 0..n {
            if lower[j] != *q {
                continue;
            }
            let mut points = MATCH;
            if is_word_start(&chars, j) {
                points += WORD_START;
            }
            if i == 0 {
                if j == 0 {
                    points += FIRST;
                }
                next[j] = Some(points);
                continue;
            }
            let previous = (0..j)
                .filter_map(|k| {
                    let base = best[k]?;
                    let bonus = if k + 1 == j {
                        CONSECUTIVE
                    } else {
                        -((j - k - 1) as i32 * GAP).min(MAX_GAP_PENALTY)
                    };
                    Some(base + bonus)
                })
                .max();
            next[j] = previous.map(|p| p + points);
        }
        best = next;
    }
    let mut total = best.into_iter().flatten().max()?;
    let query: String = query.into_iter().collect();
    let lower: String = lower.into_iter().collect();
    if lower == query {
        total += EXACT;
    } else if lower.starts_with(&query) {
        total += PREFIX;
    }
    // Shorter candidates first when the matches are equal.
    Some(total - (n - m) as i32 / 3)
}

/// True if the character at `index` starts a word: the first character, a
/// character after `_`, `-`, `.`, `:`, `/` or a space, a capital letter
/// after a small letter, or a digit after a letter.
fn is_word_start(chars: &[char], index: usize) -> bool {
    let Some(&c) = chars.get(index) else {
        return false;
    };
    let Some(&previous) = index.checked_sub(1).and_then(|i| chars.get(i)) else {
        return true;
    };
    matches!(previous, '_' | '-' | '.' | ':' | '/' | ' ')
        || (c.is_uppercase() && previous.is_lowercase())
        || (c.is_ascii_digit() && previous.is_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The candidates in the order of their scores (best first).
    fn rank<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i32, &str)> = candidates
            .iter()
            .filter_map(|c| Some((score(query, c)?, *c)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    const TYPES: &[&str] = &[
        "Fallback",
        "ForceFailure",
        "ReactiveFallback",
        "ReactiveSequence",
        "Script",
        "ScriptCondition",
        "Sequence",
        "SequenceWithMemory",
        "ServoController",
        "SetBlackboard",
        "Switch2",
        "move_base",
    ];

    #[test]
    fn characters_must_be_in_order() {
        assert!(score("seq", "Sequence").is_some());
        assert!(score("SEQ", "sequence").is_some());
        assert!(score("qes", "Sequence").is_none());
        assert!(score("xyz", "Sequence").is_none());
        assert!(score("sequences", "Sequence").is_none());
        assert_eq!(score("", "Sequence"), Some(0));
        assert_eq!(score("  ", "Sequence"), Some(0));
    }

    #[test]
    fn prefixes_and_word_starts_rank_first() {
        assert_eq!(
            rank("seq", TYPES)[..3],
            ["Sequence", "SequenceWithMemory", "ReactiveSequence"]
        );
        assert_eq!(rank("rs", TYPES)[0], "ReactiveSequence");
        assert_eq!(rank("rf", TYPES)[0], "ReactiveFallback");
        assert_eq!(rank("sc", TYPES)[..2], ["Script", "ScriptCondition"]);
        assert_eq!(rank("servo", TYPES), ["ServoController"]);
        assert_eq!(rank("mb", TYPES), ["move_base"]);
        assert_eq!(rank("sw2", TYPES), ["Switch2"]);
        assert_eq!(rank("fallback", TYPES)[0], "Fallback");
    }

    #[test]
    fn word_starts() {
        let chars: Vec<char> = "ReactiveSequence_x2".chars().collect();
        let starts: Vec<usize> = (0..chars.len())
            .filter(|i| is_word_start(&chars, *i))
            .collect();
        assert_eq!(starts, [0, 8, 17, 18]);
    }
}
