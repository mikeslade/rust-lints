//! Build-time `CARGO_MANIFEST_DIR` fixture, quiet crate: nothing here reads the
//! variable at build time, so the pass must report nothing, including the `env!`
//! calls another macro consumes (which it reports only in a crate that does read it).

use std::ffi::OsString;

pub const NAME: &str = env!("CARGO_PKG_NAME");

pub const VERSIONED: &str = concat!(env!("CARGO_PKG_NAME"), "-", env!("CARGO_PKG_VERSION"));

pub fn unset() -> Option<&'static str> {
    option_env!("RUST_LINTS_FIXTURE_UNSET_VARIABLE")
}

pub const MENTION: &str = "CARGO_MANIFEST_DIR";

pub const QUOTED: &str = stringify!(env!("CARGO_MANIFEST_DIR"));

pub fn runtime() -> Option<OsString> {
    std::env::var_os("CARGO_MANIFEST_DIR")
}
