{
  description = "rust-lints: architecture-policy Dylint library (SQL seam, HTTP wrapper, blocking-in-async, bounded channels, bool params, file length)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
    crane,
  }: let
    # The .so layout and rustc_private linking below are Linux-specific.
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      overlays = [(import rust-overlay)];
    };

    # Pinned nightly that exposes rustc_private, matching rust-toolchain. Must
    # carry rust-src + rustc-dev so the rustc_private extern crates resolve,
    # and llvm-tools-preview for the linker dylint_linking drives.
    toolchainDate = "2026-04-16";
    rustcTarget = pkgs.stdenv.hostPlatform.rust.rustcTarget;
    toolchainName = "nightly-${toolchainDate}-${rustcTarget}";
    rustToolchain = pkgs.rust-bin.nightly.${toolchainDate}.default.override {
      extensions = ["rust-src" "rustc-dev" "llvm-tools-preview"];
    };

    # Crane vendors the crate's dependencies as a fixed-output derivation, so
    # the cdylib build runs offline inside the Nix sandbox.
    craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
    src = craneLib.cleanCargoSource ./.;

    commonArgs = {
      inherit src;
      pname = "rust-lints";
      version = "0.1.0";
      strictDeps = true;
      # dylint_linking links the produced .so against the nightly's
      # rustc_private libraries; both the build and any downstream load need
      # that lib dir on the search path.
      LD_LIBRARY_PATH = "${rustToolchain}/lib";
      RUSTFLAGS = "-L ${rustToolchain}/lib";
    };

    cargoArtifacts = craneLib.buildDepsOnly commonArgs;

    dylintLib = craneLib.buildPackage (commonArgs
      // {
        inherit cargoArtifacts;
        doCheck = false;
        # Expose the cdylib with the toolchain-suffixed symlink that
        # `cargo dylint --no-build` resolves on DYLINT_LIBRARY_PATH.
        postInstall = ''
          if [ -f "$out/lib/librust_lints.so" ]; then
            ln -sf "librust_lints.so" \
              "$out/lib/librust_lints@${toolchainName}.so"
          else
            echo "rust_lints cdylib not found under $out/lib" >&2
            find "$out" -name '*.so' >&2 || true
            exit 1
          fi
        '';
      });
  in {
    packages.${system} = {
      default = dylintLib;
      dylints = dylintLib; # compatibility with the old `.#dylints` attr path
      toolchain = rustToolchain;
    };

    checks.${system}.build = dylintLib;

    devShells.${system}.default = pkgs.mkShell {
      packages = [rustToolchain pkgs.cargo-dylint pkgs.dylint-link];
      LD_LIBRARY_PATH = "${rustToolchain}/lib";
    };
  };
}
