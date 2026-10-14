//! Free TCP ports for the publisher.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Mutex, PoisonError};

/// Number of tries before [`free_port_pair`] gives up.
const MAX_TRIES: usize = 500;

/// Gives a port `p` such that `p` and `p + 1` are free on 127.0.0.1.
///
/// The publisher of BT.CPP binds the request socket on `p` and the
/// notification socket on `p + 1`. The function binds a listener to each
/// port to check it (on 127.0.0.1 and on all interfaces, because the
/// publisher binds `tcp://*:<port>`). It never gives the same port twice in
/// one process, so tests that run in parallel get different ports.
///
/// Another process can still take a port after the check. This is unlikely
/// in a test environment.
///
/// # Panics
///
/// Panics if no free pair is found after many tries.
pub fn free_port_pair() -> u16 {
    static ISSUED: Mutex<BTreeSet<u16>> = Mutex::new(BTreeSet::new());

    for _ in 0..MAX_TRIES {
        let Some(port) = ephemeral_port() else {
            continue;
        };
        let Some(next) = port.checked_add(1) else {
            continue;
        };
        let mut issued = ISSUED.lock().unwrap_or_else(PoisonError::into_inner);
        if issued.contains(&port) || issued.contains(&next) {
            continue;
        }
        if is_free(port) && is_free(next) {
            issued.insert(port);
            issued.insert(next);
            return port;
        }
    }
    panic!("free_port_pair: no free pair of TCP ports after {MAX_TRIES} tries");
}

/// Asks the operating system for a free port.
fn ephemeral_port() -> Option<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    (port != 0).then_some(port)
}

/// True if a listener can bind the port on 127.0.0.1 and on all interfaces.
fn is_free(port: u16) -> bool {
    [Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED]
        .into_iter()
        .all(|address| TcpListener::bind((address, port)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_is_free_and_bindable() {
        let port = free_port_pair();
        assert!(port > 0 && port < u16::MAX);
        let first = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let second = TcpListener::bind((Ipv4Addr::LOCALHOST, port + 1)).unwrap();
        drop((first, second));
    }

    #[test]
    fn pairs_do_not_overlap() {
        let ports: Vec<u16> = (0..20).map(|_| free_port_pair()).collect();
        let mut used = BTreeSet::new();
        for port in ports {
            assert!(used.insert(port), "port {port} given twice");
            assert!(used.insert(port + 1), "port {} given twice", port + 1);
        }
    }

    #[test]
    fn a_bound_port_is_not_free() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_free(port));
        drop(listener);
    }
}
