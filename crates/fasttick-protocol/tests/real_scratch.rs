mod real_support;
use fasttick_protocol::messages::Hook;
use fasttick_testkit::require_harness;
use real_support::*;

#[test]
fn explore() {
    let harness = require_harness!();
    let mut live = Live::start(options(harness, "long_running.xml"));
    println!("XML:\n{}", live.xml);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let bb = live.client.blackboards(&["LongRunning", "worker", "nope"]).unwrap();
    println!("BB: {}", serde_json::to_string_pretty(&bb).unwrap());
    let uid = live.uid("say_cycle");
    live.client.insert_hooks(&[Hook { enabled: false, ..Hook::breakpoint(uid) }, Hook::replace(live.uid("check_keep_going"), fasttick_core::status::NodeStatus::Success)]).unwrap();
    println!("HOOKS: {:?}", live.hooks());
    let s = live.client.status().unwrap();
    println!("STATUS: {:?}", real_support::statuses_by_name(&live.tree, &s));
    let mut probe = Live::probe(harness);
    println!("PROBE XML:\n{}", probe.xml);
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("PROBE BB {:?}", probe.client.blackboards(&["Probe"]).unwrap());
    println!("PROBE STATUS {:?}", probe.statuses());
    println!("results {:?}", probe.executor.stdout_lines().iter().take(5).collect::<Vec<_>>());
}
