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

    rustcTarget = pkgs.stdenv.hostPlatform.rust.rustcTarget;

    # The rustc_private EXTENSIONS this library needs. They are the only real
    # difference from a consumer's ordinary workspace toolchain — an extension
    # set, never a reason to be on a different nightly.
    rustcPrivateExtensions = ["rust-src" "rustc-dev" "llvm-tools-preview"];

    # Add those extensions to a given dated nightly. A consumer that pins
    # `nightly-<date>` in its own rust-toolchain.toml passes the date here and
    # gets a toolchain that is the SAME nightly plus what Dylint needs.
    toolchainForDate = date:
      pkgs.rust-bin.nightly.${date}.default.override {
        extensions = rustcPrivateExtensions;
      };

    # The toolchain used when nothing is supplied: the newest nightly in this
    # flake's own rust-overlay lock carrying every required component. This is
    # a ROLLING choice, correct only for standalone use of this repo. A consumer
    # that pins a nightly must NOT inherit it — that is how the library and the
    # consumer's workspace drift onto different compilers. Such a consumer calls
    # `lib.dylintsFor` with its own toolchain instead.
    defaultRustToolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain:
      toolchain.default.override {extensions = rustcPrivateExtensions;});

    # Dylint resolves its rustc-private driver cache by this exact name, so it
    # is derived from the toolchain rather than restated anywhere.
    toolchainNameOf = rustToolchain: let
      match = builtins.match ".*-nightly-([0-9]{4}-[0-9]{2}-[0-9]{2})" rustToolchain.version;
    in
      if match == null
      then throw "could not derive nightly date from ${rustToolchain.version}"
      else "nightly-${builtins.head match}-${rustcTarget}";

    # Everything the library build needs, as a function of the TOOLCHAIN. The
    # cdylib, the driver name Dylint dlopens it by, and the dylint tools all
    # come out of one argument, so a consumer cannot end up running a library
    # that was compiled by a different rustc than its own workspace uses.
    buildFor = rustToolchain: let
      toolchainName = toolchainNameOf rustToolchain;
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
      dylintToolsVersion = "6.1.0";
      dylintToolsSrc = pkgs.applyPatches {
        name = "dylint-${dylintToolsVersion}-source";
        src = pkgs.fetchFromGitHub {
          owner = "trailofbits";
          repo = "dylint";
          rev = "v${dylintToolsVersion}";
          hash = "sha256-KgEn3AZnITS6Uhc6CElCMqOucu+/Cc4w9Jm5oU+v5Iw=";
        };
        # rustc removed the unstable `--env-set` flag on 2026-08-31
        # (rust-lang/rust#161831). dylint >= 6.0.3 passes it on every lint
        # run to invalidate rustc's incremental cache, so its driver fails on
        # any newer nightly with "Unrecognized option: 'env-set'"
        # (trailofbits/dylint#2078). Stop passing the flag; this restores the
        # 6.0.2 behaviour, where a stale incremental cache is the caller's
        # problem (the fixture checker already lints from a cold target dir).
        # --replace-fail makes the upstream fix surface here as a build
        # failure, which is the cue to drop this patch.
        postPatch = ''
          substituteInPlace driver/src/lib.rs \
            --replace-fail 'let untracked_state = hash_from_env(&paths)?;' \
              'let _ = hash_from_env(&paths)?; let untracked_state: Option<String> = None;'
        '';
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
      inherit rustToolchain toolchainName cargoDylint dylintLink;
      dylints = dylintLib;
    };

    defaultBuild = buildFor defaultRustToolchain;
  in {
    # A consumer that pins its own nightly builds the library against THAT
    # nightly instead of inheriting this flake's rolling choice:
    #
    #   inputs.rust-lints.lib.dylintsFor { date = "2026-09-26"; }
    #
    # or, if it already has a toolchain derivation with the rustc_private
    # extensions, `dylintsFor { rustToolchain = ...; }`. Either way the returned
    # `dylints` was compiled by the same rustc the consumer runs it with, which
    # is what Dylint's `dlopen` filename check is really asserting.
    lib = {
      inherit toolchainForDate rustcPrivateExtensions;
      dylintsFor = {
        date ? null,
        rustToolchain ? null,
      }:
        if rustToolchain != null
        then buildFor rustToolchain
        else if date != null
        then buildFor (toolchainForDate date)
        else throw "dylintsFor needs either a `date` or a `rustToolchain`";
    };

    packages.${system} = {
      default = defaultBuild.dylints;
      dylints = defaultBuild.dylints; # compatibility with the old `.#dylints` attr path
      toolchain = defaultBuild.rustToolchain;
    };

    checks.${system} = {
      build = defaultBuild.dylints;
      exact-toolchain-identity = pkgs.runCommand "rust-lints-exact-toolchain-identity" {} ''
        test -L "${defaultBuild.dylints}/lib/librust_lints@${defaultBuild.toolchainName}.so"
        touch "$out"
      '';
      # The point of `dylintsFor`: a caller-supplied date must produce a library
      # named for THAT nightly, not for this flake's rolling default. Without
      # this, a consumer passing its own pin could silently be handed the
      # default build back.
      supplied-toolchain-identity = let
        supplied = buildFor (toolchainForDate "2026-09-26");
      in
        pkgs.runCommand "rust-lints-supplied-toolchain-identity" {} ''
          test -L "${supplied.dylints}/lib/librust_lints@${supplied.toolchainName}.so"
          case "${supplied.toolchainName}" in
            nightly-2026-09-26-*) ;;
            *) echo "supplied toolchain resolved to ${supplied.toolchainName}" >&2; exit 1 ;;
          esac
          touch "$out"
        '';
    };

    devShells.${system}.default = pkgs.mkShell {
      packages = [defaultBuild.rustToolchain defaultBuild.cargoDylint defaultBuild.dylintLink];
      # `cargo dylint` compiles its per-toolchain driver at runtime; the
      # driver's dep tree includes openssl-sys.
      nativeBuildInputs = [pkgs.pkg-config];
      buildInputs = [pkgs.openssl];
      LD_LIBRARY_PATH = "${defaultBuild.rustToolchain}/lib";
      # No rustup in the shell: dylint-link and `cargo dylint` resolve the
      # toolchain via RUSTUP_TOOLCHAIN and the rustup-shim wrappers.
      RUSTUP_TOOLCHAIN = defaultBuild.toolchainName;
      RUST_LINTS_RUSTUP_TOOLCHAIN = defaultBuild.toolchainName;
      RUST_LINTS_CARGO = "${defaultBuild.rustToolchain}/bin/cargo";
      RUST_LINTS_RUSTC = "${defaultBuild.rustToolchain}/bin/rustc";
      RUST_LINTS_RUSTDOC = "${defaultBuild.rustToolchain}/bin/rustdoc";
      shellHook = ''
        export PATH="$PWD/rustup-shim:$PATH"
      '';
    };
  };
}
