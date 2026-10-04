//! Fixtures for the env-gated conversion / channel / bool-param policy passes.
//!
//! Each pass below has a VIOLATION case (fires only when its `RUST_LINTS_*` var
//! is set) and a CLEAN case (never fires). With all gates UNSET this whole crate
//! is silent.
//!
//! Every silent-saturation finding carries a trailing `// silent-saturation: expect`
//! marker; `scripts/check-silent-saturation-fixture.sh` set-diffs the markers against
//! what the pass reports, in both directions.

use std::convert::identity;

// ---------------------------------------------------------------------------
// Pass: silent numeric saturation / default substitution
// gate: RUST_LINTS_SILENT_SATURATION
// ---------------------------------------------------------------------------

// VIOLATION: a fallible numeric conversion whose overflow is silently saturated.
pub fn saturating_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX) // silent-saturation: expect
}

// VIOLATION: `.unwrap_or_default()` swallows the out-of-range case to 0.
pub fn defaulting_count(count: u64) -> u32 {
    u32::try_from(count).unwrap_or_default() // silent-saturation: expect
}

// VIOLATION: `.try_into()` receiver form, error discarded by `unwrap_or_else`.
pub fn truncating_len(len: usize) -> u16 {
    let narrowed: u16 = len.try_into().unwrap_or_else(|_| u16::MAX); // silent-saturation: expect
    narrowed
}

// VIOLATION (#1934): `map_or` with an identity closure is `unwrap_or`.
pub fn identity_closure(x: u64) -> u32 {
    u32::try_from(x).map_or(u32::MAX, |v| v) // silent-saturation: expect
}

// VIOLATION: the identity function, however it is spelled or imported.
pub fn identity_fn_std(x: u64) -> u32 {
    u32::try_from(x).map_or(u32::MAX, std::convert::identity) // silent-saturation: expect
}

pub fn identity_fn_core(x: u64) -> u32 {
    u32::try_from(x).map_or(u32::MAX, core::convert::identity) // silent-saturation: expect
}

pub fn identity_fn_imported(x: u64) -> u32 {
    u32::try_from(x).map_or(u32::MAX, identity) // silent-saturation: expect
}

// VIOLATION: a typed parameter and a block body are still the identity, and any
// default counts, as it does for `unwrap_or`.
pub fn identity_block(x: u64) -> u32 {
    u32::try_from(x).map_or(0, |v: u32| { v }) // silent-saturation: expect
}

// VIOLATION: `map_or_else` with an identity mapping is `unwrap_or_else`.
pub fn identity_map_or_else(len: usize) -> u16 {
    u16::try_from(len).map_or_else(|_| u16::MAX, |v| v) // silent-saturation: expect
}

// VIOLATION: the `.try_into()` receiver form.
pub fn identity_try_into(x: i64) -> i32 {
    x.try_into().map_or(i32::MIN, |v: i32| v) // silent-saturation: expect
}

// VIOLATION: a cast to the type the value already has changes nothing.
pub fn identity_cast(x: u32) -> u8 {
    u8::try_from(x).map_or(u8::MAX, |v| v as u8) // silent-saturation: expect
}

// CLEAN: `map_or(None, Some)` is `.ok()`; the error is surfaced as `None`.
pub fn surfaced_as_none(x: u64) -> Option<u32> {
    u32::try_from(x).map_or(None, Some)
}

// CLEAN: a mapping that is not the identity is not proven to be `unwrap_or`.
pub fn mapped_value(x: u64) -> u32 {
    u32::try_from(x).map_or(0, |v| v / 2)
}

pub fn predicate(x: u64) -> bool {
    u32::try_from(x).map_or(false, |v| v > 3)
}

// CLEAN: a closure that returns a captured value, not its parameter.
pub fn captured_fallback(x: u64, fallback: u32) -> u32 {
    u32::try_from(x).map_or(u32::MAX, |_v| fallback)
}

// CLEAN: a cast to another type is a conversion, not the identity.
pub fn widened(x: u64) -> u64 {
    u32::try_from(x).map_or(0, |v| v as u64)
}

// CLEAN: a non-numeric `TryFrom` (target is a String), so the numeric gate keeps
// the pass quiet even though the unwrap_or shape is present.
pub fn lossy_string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_default()
}

// CLEAN: the identity over a non-numeric conversion.
pub fn lossy_string_identity(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).map_or(String::new(), |s| s)
}

// CLEAN: the out-of-range case is handled explicitly rather than swallowed.
pub fn checked_count(count: usize) -> Option<i32> {
    match i32::try_from(count) {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Pass: unbounded channel
// gate: RUST_LINTS_UNBOUNDED_CHANNEL
// ---------------------------------------------------------------------------

// Local stand-in modules mirroring the real channel constructor paths. The lint
// matches on the module-qualified path suffix, so these resolve like the real
// `tokio::sync::mpsc::unbounded_channel` etc. at the fixture's def paths.
pub mod tokio {
    pub mod sync {
        pub mod mpsc {
            pub fn unbounded_channel() {}
            pub fn channel(_capacity: usize) {}
        }
    }
}

pub mod crossbeam_channel {
    pub fn unbounded() {}
    pub fn bounded(_capacity: usize) {}
}

// VIOLATION: unbounded constructor with no backpressure bound.
pub fn spawn_unbounded() {
    tokio::sync::mpsc::unbounded_channel();
    crossbeam_channel::unbounded();
}

// CLEAN: bounded constructors sized to the workload.
pub fn spawn_bounded() {
    tokio::sync::mpsc::channel(64);
    crossbeam_channel::bounded(64);
}

// ---------------------------------------------------------------------------
// Pass: boolean parameter on a public fn
// gate: RUST_LINTS_BOOL_PARAMS
// ---------------------------------------------------------------------------

// VIOLATION: public free fn with a blind bool parameter.
pub fn render_report(_verbose: bool) {}

// VIOLATION: public inherent method with a bool parameter.
pub struct Renderer;

impl Renderer {
    pub fn render(&self, _compact: bool) {}
}

// CLEAN: a private fn is not a public-surface concern.
fn private_render(_verbose: bool) {}

// CLEAN: `set_*` / `with_*` setters read fine at the call site already.
impl Renderer {
    pub fn set_compact(&mut self, _compact: bool) {}
    pub fn with_verbose(self, _verbose: bool) -> Self {
        self
    }
}

// CLEAN: no bool parameter at all.
pub fn render_level(_level: u8) {}

// ---------------------------------------------------------------------------
// Suppression still works: an explicit allow silences the new messages.
// ---------------------------------------------------------------------------

#[allow(rust_lints_policy_checks)]
pub fn suppressed_saturation(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

#[allow(rust_lints_policy_checks)]
pub fn suppressed_identity_saturation(count: usize) -> i32 {
    i32::try_from(count).map_or(i32::MAX, |v| v)
}

#[allow(rust_lints_policy_checks)]
pub fn suppressed_bool_param(_verbose: bool) {}

// ---------------------------------------------------------------------------
// Test items are skipped by the bool-param pass.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // CLEAN under #[cfg(test)]: bool params in test helpers are not flagged.
    pub fn assert_rendered(_strict: bool) {}

    // CLEAN under #[cfg(test)]: the silent-saturation pass skips the test-crate
    // build, so a saturating conversion in a fixture (no persisted/wire value to
    // corrupt) is not flagged even though the `unwrap_or` shape is present. This
    // is the negative control for the test-code skip — if the pass ever fired in
    // test code again, this line would flag and the corpus run would fail.
    pub fn fixture_narrow(count: usize) -> i32 {
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    pub fn fixture_narrow_identity(count: usize) -> i32 {
        i32::try_from(count).map_or(i32::MAX, |v| v)
    }

    #[test]
    fn renders() {
        assert_rendered(true);
        let _ = fixture_narrow(0);
        let _ = fixture_narrow_identity(0);
    }
}
