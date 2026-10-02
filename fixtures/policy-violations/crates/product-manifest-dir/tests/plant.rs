//! A test target is linted like any other: the read bakes the checkout just the same.

#[test]
fn plant() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")); // manifest-dir: expect
    assert!(root.is_absolute());
}
