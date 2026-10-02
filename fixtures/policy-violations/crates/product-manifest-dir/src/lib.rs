//! Build-time `CARGO_MANIFEST_DIR` fixture: reads the pass must refuse, next to code
//! that only MENTIONS the variable or reads another one.
//!
//! A line the pass must refuse carries a trailing marker: `manifest-dir: expect` for a
//! read it proves from the literal `env!` left behind, `manifest-dir: expect-consumed`
//! for a call whose literal another macro swallowed. Everything unmarked must stay quiet.
//! `scripts/check-manifest-dir-fixture.sh` set-diffs the markers against the run.
//!
//! The first three modules are the shapes radiology-platform #2666 found getting past
//! a token scan: a `use` of `env` under another name that a `macro_rules!` assembled,
//! called with a variable name another macro builds.

use std::path::Path;

/// Shape 1: the macro name arrives as an `ident` metavariable.
pub mod ident_metavariable {
    macro_rules! m {
        ($x:ident) => {
            use std::$x as r;
        };
    }
    m!(env);
    macro_rules! name {
        () => {
            "CARGO_MANIFEST_DIR"
        };
    }

    pub fn read() -> &'static str {
        r!(name!()) // manifest-dir: expect
    }
}

/// Shape 2: the macro path arrives as a `path` metavariable.
pub mod path_metavariable {
    macro_rules! m {
        ($p:path) => {
            use $p as r;
        };
    }
    m!(std::env);
    macro_rules! name {
        () => {
            "CARGO_MANIFEST_DIR"
        };
    }

    pub fn read() -> &'static str {
        r!(name!()) // manifest-dir: expect
    }
}

/// Shape 3: the whole `use` tree is forwarded as token trees.
pub mod token_tree_forwarding {
    macro_rules! m {
        ($($t:tt)*) => {
            use $($t)*;
        };
    }
    m!(std::env as r);
    macro_rules! name {
        () => {
            "CARGO_MANIFEST_DIR"
        };
    }

    pub fn read() -> &'static str {
        r!(name!()) // manifest-dir: expect
    }
}

/// The plain spellings a token scan also catches.
pub mod plain {
    use std::path::Path;

    pub const ROOT: &str = env!("CARGO_MANIFEST_DIR"); // manifest-dir: expect

    pub fn path() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR")) // manifest-dir: expect
    }

    pub fn optional() -> Option<&'static str> {
        option_env!("CARGO_MANIFEST_DIR") // manifest-dir: expect
    }

    pub fn core_path() -> &'static str {
        core::env!("CARGO_MANIFEST_DIR", "cargo sets it") // manifest-dir: expect
    }

    pub fn std_path_brackets() -> &'static str {
        std::env!["CARGO_MANIFEST_DIR"] // manifest-dir: expect
    }

    pub fn escaped_name() -> &'static str {
        env!("CARGO_MANIFEST_\u{44}IR") // manifest-dir: expect
    }

    pub fn concat_built_name() -> &'static str {
        env!(concat!("CARGO_", "MANIFEST_DIR")) // manifest-dir: expect
    }
}

/// A plain rename, with no macro involved in building it.
pub mod renamed {
    use std::env as renamed;
    use std::option_env as maybe;

    pub fn read() -> &'static str {
        renamed!("CARGO_MANIFEST_DIR") // manifest-dir: expect
    }

    pub fn optional() -> Option<&'static str> {
        maybe!("CARGO_MANIFEST_DIR") // manifest-dir: expect
    }
}

/// The read sits in a local macro's body; it is reported where the `env!` is written.
pub mod in_macro_body {
    macro_rules! root {
        () => {
            env!("CARGO_MANIFEST_DIR") // manifest-dir: expect
        };
    }

    pub fn read() -> &'static str {
        root!()
    }
}

/// The read sits in a macro another crate exports. rustc drops a lint inside a foreign
/// macro, so it is reported at the invocation here.
pub mod foreign_macro {
    pub fn read() -> &'static str {
        fixture_product_manifest_dir_clean::manifest_dir!() // manifest-dir: expect
    }
}

/// A call whose literal another builtin consumed: nothing is left to compare, and the
/// crate reads the variable, so the call is refused.
pub mod consumed {
    pub const MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"); // manifest-dir: expect-consumed

    pub const CONTENTS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")); // manifest-dir: expect-consumed
}

/// Quiet: these mention `CARGO_MANIFEST_DIR` or read another variable, in a crate that
/// does read it, so they also prove the pass compares the value and not the crate.
pub mod quiet {
    use std::ffi::OsString;

    /// Another variable cargo sets; its literal does not hold the manifest dir.
    pub const NAME: &str = env!("CARGO_PKG_NAME");

    /// An unset variable: `option_env!` leaves `None`, not the manifest dir.
    pub fn unset() -> Option<&'static str> {
        option_env!("RUST_LINTS_FIXTURE_UNSET_VARIABLE")
    }

    /// A string that is not inside `env!`.
    pub const MENTION: &str = "CARGO_MANIFEST_DIR";

    /// `stringify!` never expands its input.
    pub const QUOTED: &str = stringify!(env!("CARGO_MANIFEST_DIR"));

    /// The run-time read the pass exists to steer people to.
    pub fn runtime() -> Option<OsString> {
        std::env::var_os("CARGO_MANIFEST_DIR")
    }
}

pub fn uses() -> usize {
    let _ = Path::new(quiet::MENTION);
    ident_metavariable::read().len()
        + path_metavariable::read().len()
        + token_tree_forwarding::read().len()
        + renamed::read().len()
        + in_macro_body::read().len()
}
