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

    # Select the newest nightly in the locked rust-overlay snapshot that carries
    # every required component. Updating flake.lock rolls this forward; the lock
    # keeps the selected toolchain reproducible between updates.
    rustcTarget = pkgs.stdenv.hostPlatform.rust.rustcTarget;
    rustToolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain:
      toolchain.default.override {
        extensions = ["rust-src" "rustc-dev" "llvm-tools-preview"];
      });
    toolchainDateMatch =
      builtins.match ".*-nightly-([0-9]{4}-[0-9]{2}-[0-9]{2})" rustToolchain.version;
    toolchainDate =
      if toolchainDateMatch == null
      then throw "could not derive nightly date from ${rustToolchain.version}"
      else builtins.head toolchainDateMatch;
    # Dylint caches a rustc-private driver by this name, so retain the exact
    # resolved date even though toolchain selection itself is rolling.
    toolchainName = "nightly-${toolchainDate}-${rustcTarget}";

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

    # cargo-dylint and dylint-link are not packaged in nixpkgs, so the
    # devshell builds them with the same resolved toolchain. Built from the
    # dylint repo rather than crates.io: dylint's build.rs packages the
    # sibling `driver/` directory, which the crates.io tarball omits.
    # Version matches the dylint_linting pin in Cargo.lock.
    dylintToolsVersion = "6.0.2";
    dylintToolsSrc = pkgs.fetchFromGitHub {
      owner = "trailofbits";
      repo = "dylint";
      rev = "v${dylintToolsVersion}";
      hash = "sha256-cAN2eXOlWcmhD4co+8Wo7zEfwPnIF+RKoexdvpCRcrc=";
    };
    dylintToolsArgs = {
      pname = "dylint-tools";
      version = dylintToolsVersion;
      src = dylintToolsSrc;
      doCheck = false;
      nativeBuildInputs = [pkgs.pkg-config];
      buildInputs = [pkgs.openssl];
    };
    dylintToolsArtifacts = craneLib.buildDepsOnly dylintToolsArgs;
    buildDylintTool = pname:
      craneLib.buildPackage (dylintToolsArgs
        // {
          inherit pname;
          cargoArtifacts = dylintToolsArtifacts;
          cargoExtraArgs = "-p ${pname}";
          # dylint's build.rs bakes an absolute path to the driver sources
          # (used at runtime to build per-toolchain drivers); the sandbox
          # path dies with the build, so point it at the store copy, which
          # the baked reference then keeps alive.
          postPatch = ''
            substituteInPlace dylint/build.rs \
              --replace-fail 'dylint_manifest_dir.join("../driver")' \
                'std::path::PathBuf::from("${dylintToolsSrc}/driver")'
          '';
        });
    cargoDylint = buildDylintTool "cargo-dylint";
    dylintLink = buildDylintTool "dylint-link";

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

    checks.${system} = {
      build = dylintLib;
      exact-toolchain-identity = pkgs.runCommand "rust-lints-exact-toolchain-identity" {} ''
        test -L "${dylintLib}/lib/librust_lints@${toolchainName}.so"
        touch "$out"
      '';
    };

    devShells.${system}.default = pkgs.mkShell {
      packages = [rustToolchain cargoDylint dylintLink];
      # `cargo dylint` compiles its per-toolchain driver at runtime; the
      # driver's dep tree includes openssl-sys.
      nativeBuildInputs = [pkgs.pkg-config];
      buildInputs = [pkgs.openssl];
      LD_LIBRARY_PATH = "${rustToolchain}/lib";
      # No rustup in the shell: dylint-link and `cargo dylint` resolve the
      # toolchain via RUSTUP_TOOLCHAIN and the rustup-shim wrappers.
      RUSTUP_TOOLCHAIN = toolchainName;
      RUST_LINTS_RUSTUP_TOOLCHAIN = toolchainName;
      RUST_LINTS_CARGO = "${rustToolchain}/bin/cargo";
      RUST_LINTS_RUSTC = "${rustToolchain}/bin/rustc";
      RUST_LINTS_RUSTDOC = "${rustToolchain}/bin/rustdoc";
      shellHook = ''
        export PATH="$PWD/rustup-shim:$PATH"
      '';
    };
  };
}
