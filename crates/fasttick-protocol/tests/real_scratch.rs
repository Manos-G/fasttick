mod real_support;
use std::time::{Duration, Instant};
use fasttick_core::status::NodeStatus;
use fasttick_protocol::messages::Hook;
use fasttick_testkit::{require_harness, Executor};
use real_support::*;

#[test]
fn explore_sub_after_restart() {
    let harness = require_harness!();
    let opts = options(harness, "probe.xml");
    let port = opts.port;
    let mut live = Live::start(opts.clone());
    let runs = live.uid("count_runs");
    live.sync_notifications(runs);
    live.executor.stop().unwrap();
    let t0 = Instant::now();
    let _b = Executor::spawn(opts).unwrap();
    println!("B ready after {:?}", t0.elapsed());
    // Old client: REQ may need a reset after timeouts.
    live.client.set_request_timeout(Duration::from_millis(500));
    loop {
        match live.client.insert_hook(&Hook::replace(runs, NodeStatus::Skipped)) {
            Ok(()) => break,
            Err(e) => println!("insert failed at {:?}: {e}", t0.elapsed()),
        }
    }
    println!("hook inserted at {:?}", t0.elapsed());
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(n) = live.client.poll_notification(Duration::from_millis(100)).unwrap() {
            println!("first notification {:?} at {:?}", n.uid, t0.elapsed());
            return;
        }
    }
    println!("no notification within 10 s");
}
