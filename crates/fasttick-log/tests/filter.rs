//! Tests of the transition filter.

mod support;

use std::collections::BTreeSet;

use fasttick_core::runtime::{RuntimeTree, Uid};
use fasttick_core::status::NodeStatus;
use fasttick_log::btlog::Transition;
use fasttick_log::filter::TransitionFilter;
use support::{SAMPLE_XML, t};

fn tree() -> RuntimeTree {
    RuntimeTree::parse(SAMPLE_XML).unwrap()
}

/// One RUNNING transition for each node of the sample tree, plus a UID
/// that is not in the tree.
fn all_nodes() -> Vec<Transition> {
    (1..=8)
        .chain([99])
        .map(|uid| t(u64::from(uid) * 10, uid, NodeStatus::Running))
        .collect()
}

fn matching_uids(filter: &TransitionFilter) -> Vec<u16> {
    let transitions = all_nodes();
    filter
        .apply(&tree(), &transitions)
        .into_iter()
        .map(|position| transitions[position].uid.0)
        .collect()
}

fn include(patterns: &[&str]) -> TransitionFilter {
    TransitionFilter {
        include: patterns.iter().map(|p| p.to_string()).collect(),
        ..TransitionFilter::default()
    }
}

#[test]
fn empty_filter_accepts_all() {
    let filter = TransitionFilter::new();
    assert!(filter.is_empty());
    assert_eq!(matching_uids(&filter), vec![1, 2, 3, 4, 5, 6, 7, 8, 99]);
}

#[test]
fn include_by_name_path_and_type() {
    // Name, substring, any case.
    assert_eq!(matching_uids(&include(&["GOAL"])), vec![2]);
    // Type ID.
    assert_eq!(matching_uids(&include(&["approachdock"])), vec![6]);
    // Path: all nodes of the SubTree instance, and the SubTree node itself.
    assert_eq!(matching_uids(&include(&["dock_sub"])), vec![4, 5, 6, 7, 8]);
    // Several patterns: any one is enough.
    assert_eq!(matching_uids(&include(&["move", "say"])), vec![2, 3]);
    // No match.
    assert_eq!(matching_uids(&include(&["nothing"])), Vec::<u16>::new());
}

#[test]
fn wildcards() {
    // Anchored at both ends when there are wildcards.
    assert_eq!(matching_uids(&include(&["Always*"])), vec![7, 8]);
    assert_eq!(matching_uids(&include(&["*fallback"])), vec![5]);
    assert_eq!(matching_uids(&include(&["dock_sub/*"])), vec![5, 6, 7, 8]);
    assert_eq!(matching_uids(&include(&["?ove*"])), vec![2]);
    assert_eq!(matching_uids(&include(&["main_se?"])), vec![1]);
    assert_eq!(matching_uids(&include(&["main_s?"])), Vec::<u16>::new());
    // `*` alone matches all known nodes, but not the unknown UID.
    assert_eq!(
        matching_uids(&include(&["*"])),
        vec![1, 2, 3, 4, 5, 6, 7, 8]
    );
    // Special characters in names.
    assert_eq!(matching_uids(&include(&["say, \"hi\""])), vec![3]);
    assert_eq!(matching_uids(&include(&["two?lines"])), vec![8]);
}

#[test]
fn exclude_wins_over_include() {
    let filter = TransitionFilter {
        include: vec!["dock_sub".into()],
        exclude: vec!["always*".into()],
        ..TransitionFilter::default()
    };
    assert_eq!(matching_uids(&filter), vec![4, 5, 6]);

    let filter = TransitionFilter {
        exclude: vec!["Sequence".into(), "dock".into()],
        ..TransitionFilter::default()
    };
    // The unknown UID passes: include is empty and no pattern can exclude it.
    assert_eq!(matching_uids(&filter), vec![2, 3, 99]);

    let filter = TransitionFilter {
        include: vec!["move".into()],
        exclude: vec!["move".into()],
        ..TransitionFilter::default()
    };
    assert_eq!(matching_uids(&filter), Vec::<u16>::new());
}

#[test]
fn status_and_time_range() {
    let transitions = vec![
        t(10, 1, NodeStatus::Running),
        t(20, 2, NodeStatus::Success),
        t(30, 2, NodeStatus::Idle),
        t(40, 1, NodeStatus::Failure),
        t(50, 6, NodeStatus::Skipped),
    ];
    let tree = tree();

    let filter = TransitionFilter {
        statuses: Some(BTreeSet::from([NodeStatus::Success, NodeStatus::Failure])),
        ..TransitionFilter::default()
    };
    assert_eq!(filter.apply(&tree, &transitions), vec![1, 3]);

    let filter = TransitionFilter {
        time_range: Some((20, 40)),
        ..TransitionFilter::default()
    };
    assert_eq!(filter.apply(&tree, &transitions), vec![1, 2, 3]);

    // A reversed range works the same.
    let filter = TransitionFilter {
        time_range: Some((40, 20)),
        ..TransitionFilter::default()
    };
    assert_eq!(filter.apply(&tree, &transitions), vec![1, 2, 3]);

    // All parts together.
    let filter = TransitionFilter {
        include: vec!["main*".into(), "move*".into()],
        exclude: vec![],
        statuses: Some(BTreeSet::from([NodeStatus::Failure, NodeStatus::Idle])),
        time_range: Some((25, 100)),
    };
    assert_eq!(filter.apply(&tree, &transitions), vec![2, 3]);

    // An empty status set accepts nothing.
    let filter = TransitionFilter {
        statuses: Some(BTreeSet::new()),
        ..TransitionFilter::default()
    };
    assert!(filter.apply(&tree, &transitions).is_empty());
}

#[test]
fn compiled_filter() {
    let filter = include(&["dock_sub/*"]);
    assert!(!filter.is_empty());
    let compiled = filter.compile(&tree());
    assert!(compiled.node_matches(Uid(5)));
    assert!(!compiled.node_matches(Uid(1)));
    assert!(!compiled.node_matches(Uid(99)));
    assert!(compiled.matches(&t(0, 6, NodeStatus::Idle)));
}

#[test]
fn filter_serde_round_trip() {
    let filter = TransitionFilter {
        include: vec!["a*".into()],
        exclude: vec!["b".into()],
        statuses: Some(BTreeSet::from([NodeStatus::Running])),
        time_range: Some((1, 2)),
    };
    let json = serde_json::to_string(&filter).unwrap();
    assert!(json.contains("\"RUNNING\""), "{json}");
    let back: TransitionFilter = serde_json::from_str(&json).unwrap();
    assert_eq!(back, filter);
}

/// A slow, plain matcher for `*` and `?` that matches the full text.
fn naive_glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| naive_glob(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && naive_glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && naive_glob(rest, &text[1..]),
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    #[test]
    fn prop_pattern_matches_like_a_naive_matcher(
        pattern in "[aAbB*?]{1,8}",
        text in "[aAbB]{0,10}",
    ) {
        let compiled = fasttick_log::filter::Pattern::new(&pattern).unwrap();
        let pattern_chars: Vec<char> = pattern.to_lowercase().chars().collect();
        let text_lower = text.to_lowercase();
        let text_chars: Vec<char> = text_lower.chars().collect();
        let expected = if pattern.contains(['*', '?']) {
            naive_glob(&pattern_chars, &text_chars)
        } else {
            text_lower.contains(&pattern.to_lowercase())
        };
        proptest::prop_assert_eq!(compiled.matches(&text), expected);
    }
}
