//! A build script is linted too; its `env!` bakes the checkout into the script binary.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let root = env!("CARGO_MANIFEST_DIR"); // manifest-dir: expect
    assert!(!root.is_empty());
}
