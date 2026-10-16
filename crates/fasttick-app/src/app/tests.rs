use super::*;
use fasttick_core::runtime::RuntimeTree;
use fasttick_protocol::breakpoints::Breakpoint;

#[test]
fn endpoints() {
    assert_eq!(parse_endpoint("robot"), Some(("robot".into(), 1667)));
    assert_eq!(parse_endpoint(" robot:1700 "), Some(("robot".into(), 1700)));
    assert_eq!(parse_endpoint("[::1]:1800"), Some(("::1".into(), 1800)));
    assert_eq!(parse_endpoint("[::1]"), Some(("::1".into(), 1667)));
    assert_eq!(parse_endpoint("::1"), Some(("::1".into(), 1667)));
    assert_eq!(parse_endpoint(""), None);
    assert_eq!(parse_endpoint(":1700"), None);
    assert_eq!(parse_endpoint("robot:0"), None);
    assert_eq!(parse_endpoint("robot:x"), None);
    assert_eq!(parse_endpoint("[::1]x"), None);
}

#[test]
fn pending_breakpoints_choose_the_nodes_found_by_path() {
    let tree = RuntimeTree::parse(
        r#"<root><BehaviorTree ID="T" _fullpath=""><Sequence name="s" _uid="1"><AlwaysSuccess name="a" _uid="2"/><AlwaysSuccess name="a" _uid="3"/></Sequence></BehaviorTree></root>"#,
    )
    .unwrap();
    let breakpoint = |path: &str, uid: Option<u16>| Breakpoint {
        node_path: path.into(),
        uid,
        position: Default::default(),
        mode: HookMode::Breakpoint,
        enabled: true,
        once: false,
        desired_status: NodeStatus::Skipped,
        label: None,
    };
    let mut set = BreakpointSet::new();
    set.breakpoints = vec![
        breakpoint("s", Some(9)),
        breakpoint("a", Some(3)),
        breakpoint("gone", Some(2)),
        breakpoint("gone", Some(77)),
    ];
    let pending = PendingBreakpoints::new("x.ftbp.json".into(), set, &tree);
    let resolutions: Vec<Resolution> = pending.resolved.iter().map(|r| r.resolution).collect();
    assert_eq!(
        resolutions,
        [
            Resolution::ByPath,
            Resolution::AmbiguousPath,
            Resolution::ByStoredUid,
            Resolution::Unresolved
        ]
    );
    assert_eq!(pending.chosen, [true, true, false, false]);
    let uids: Vec<u16> = pending.hooks().iter().map(|h| h.uid).collect();
    assert_eq!(uids, [1, 3]);
    for (resolved, stored) in pending.resolved.iter().zip([9, 3, 2, 77]) {
        let (text, color) = resolution_text(resolved, Some(stored));
        assert!(!text.is_empty());
        match resolved.resolution {
            Resolution::ByPath => assert_eq!(color, ResolutionColor::Good),
            Resolution::Unresolved => assert_eq!(color, ResolutionColor::Bad),
            _ => assert_eq!(color, ResolutionColor::Warning),
        }
    }
    assert!(
        resolution_text(&pending.resolved[2], Some(2))
            .0
            .contains("UID 2")
    );
}
