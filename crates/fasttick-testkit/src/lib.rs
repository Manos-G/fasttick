//! Test support: builds and starts the BT.CPP test executor in `test_harness/`.
//!
//! This crate is for tests only. It is not published.
//!
//! The main items:
//!
//! - [`btcpp_prefix`] finds an install of BehaviorTree.CPP 4.x.
//! - [`harness`] builds the C++ harness with CMake, once for each process
//!   and once at a time across processes.
//! - [`require_harness!`] skips a test cleanly when BT.CPP is not available.
//! - [`Executor`] starts `fasttick_executor`, reads its output (for
//!   example [`Executor::wait_for_stdout`]) and stops it with a signal.
//! - [`free_port_pair`] gives a port `p` where `p` and `p + 1` are free.
//! - [`TempDir`] is a temporary directory that is removed on drop.
//!
//! # Example
//!
//! ```no_run
//! use fasttick_testkit::{require_harness, Executor, ExecutorOptions};
//!
//! #[test]
//! fn runs_main_tree() {
//!     let harness = require_harness!();
//!     let mut options = ExecutorOptions::new(harness.tree("main.xml"));
//!     options.loops = 1;
//!     let mut executor = Executor::spawn(options).unwrap();
//!     executor.wait_done(std::time::Duration::from_secs(30)).unwrap();
//! }
//! ```
//!
//! # Environment variables
//!
//! - `FASTTICK_BTCPP_PREFIX`: the BT.CPP install prefix (the directory that
//!   contains `lib/cmake/behaviortree_cpp` or `share/behaviortree_cpp/cmake`).
//! - `FASTTICK_REQUIRE_HARNESS=1`: a test fails (instead of a skip) when the
//!   harness is not available. Use it in CI jobs that install BT.CPP.

mod executor;
mod harness;
mod net;
mod prefix;
mod temp;

pub use executor::{Executor, ExecutorOptions, Signal};
pub use harness::{Harness, SkipReason, fixture_path, harness, skip_message, workspace_root};
pub use net::free_port_pair;
pub use prefix::{DEFAULT_PREFIXES, PREFIX_ENV, REQUIRE_ENV, btcpp_prefix, find_btcpp_prefix};
pub use temp::TempDir;

/// Gets the test harness, or skips the current test.
///
/// The macro calls [`harness()`]. If the harness is available, the macro
/// gives `&'static Harness`. If not, it writes
/// `SKIPPED: BT.CPP 4.x not found (set FASTTICK_BTCPP_PREFIX) — <reason>`
/// to stderr and returns from the test function.
///
/// - `require_harness!()` returns `()`. Use it in a test that returns `()`.
/// - `require_harness!(value)` returns `value`, for example
///   `require_harness!(Ok(()))` in a test that returns a `Result`.
///
/// The macro panics (the test fails) if BT.CPP was found but the harness
/// does not build, or if `FASTTICK_REQUIRE_HARNESS=1` is set. See
/// [`SkipReason::is_fatal`] for the exact rules.
#[macro_export]
macro_rules! require_harness {
    () => {
        $crate::require_harness!(())
    };
    ($ret:expr) => {
        match $crate::harness() {
            ::std::result::Result::Ok(harness) => harness,
            ::std::result::Result::Err(reason) => {
                if reason.is_fatal() {
                    ::std::panic!("the FastTick test harness is not usable: {reason}");
                }
                ::std::eprintln!("{}", $crate::skip_message(&reason));
                return $ret;
            }
        }
    };
}
