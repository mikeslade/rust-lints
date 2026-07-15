//! Fixtures for the env-gated conversion / channel / bool-param policy passes.
//!
//! Each pass below has a VIOLATION case (fires only when its `RUST_LINTS_*` var
//! is set) and a CLEAN case (never fires). With all gates UNSET this whole crate
//! is silent.

// ---------------------------------------------------------------------------
// Pass: silent numeric saturation / default substitution
// gate: RUST_LINTS_SILENT_SATURATION
// ---------------------------------------------------------------------------

// VIOLATION: a fallible numeric conversion whose overflow is silently saturated.
pub fn saturating_count(count: usize) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

// VIOLATION: `.unwrap_or_default()` swallows the out-of-range case to 0.
pub fn defaulting_count(count: u64) -> u32 {
    u32::try_from(count).unwrap_or_default()
}

// VIOLATION: `.try_into()` receiver form, error discarded by `unwrap_or_else`.
pub fn truncating_len(len: usize) -> u16 {
    let narrowed: u16 = len.try_into().unwrap_or_else(|_| u16::MAX);
    narrowed
}

// CLEAN: a non-numeric `TryFrom` (target is a String), so the numeric gate keeps
// the pass quiet even though the unwrap_or shape is present.
pub fn lossy_string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_default()
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
pub fn suppressed_bool_param(_verbose: bool) {}

// ---------------------------------------------------------------------------
// Test items are skipped by the bool-param pass.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // CLEAN under #[cfg(test)]: bool params in test helpers are not flagged.
    pub fn assert_rendered(_strict: bool) {}

    #[test]
    fn renders() {
        assert_rendered(true);
    }
}
