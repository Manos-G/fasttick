//! The vendored ZeroMQ library calls Windows security functions
//! (`InitializeSecurityDescriptor`, `SetSecurityDescriptorDacl`), but the
//! zmq crates do not tell the linker about `advapi32`. This script adds it,
//! so programs and tests that use this crate link on Windows.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
