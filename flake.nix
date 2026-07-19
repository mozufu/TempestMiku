{
  description = "TempestMiku — code-execution agent runtime and Flutter client contracts";

  inputs = {
    # Flutter on 26.05 carries Dart 3.9+, required by the maintained Markdown renderer.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
    }:
    (flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Pin a recent stable toolchain. Edition 2024 needs rustc >= 1.85; `stable.latest`
        # tracks the newest stable the overlay knows, well past that.
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
        };
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };
        tmServer = rustPlatform.buildRustPackage {
          pname = "tm-server";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "-p"
            "tm-server"
          ];
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];
          doCheck = false;
        };
        tmWorker = rustPlatform.buildRustPackage {
          pname = "tm-worker";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "-p"
            "tm-worker"
          ];
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];
          doCheck = false;
        };
        m4IsolationRuntime = pkgs.pkgsStatic.stdenv.mkDerivation {
          pname = "tempestmiku-m4-isolation-runtime";
          version = "1";
          src = pkgs.lib.cleanSource ./tools;
          nativeBuildInputs = [ pkgs.pkgsStatic.stdenv.cc ];
          dontConfigure = true;
          buildPhase = ''
            runHook preBuild
            $CC -O2 -static -pthread m4-thread-probe.c -o thread-probe
            $CC -O2 -static m4-resource-probe.c -o resource-probe
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            mkdir -p "$out/bin"
            cp ${pkgs.bubblewrap}/bin/bwrap "$out/bin/bwrap"
            cp ${pkgs.pkgsStatic.busybox}/bin/busybox "$out/bin/busybox"
            install -m 0555 thread-probe resource-probe "$out/bin/"
            for applet in cat env mount sh sleep test touch true unshare wget; do
              ln -s busybox "$out/bin/$applet"
            done
            runHook postInstall
          '';
        };
      in
      {
        packages = {
          inherit tmServer tmWorker;
          default = tmServer;
          "tm-server" = tmServer;
          "tm-worker" = tmWorker;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          inherit m4IsolationRuntime;
          "m4-isolation-runtime" = m4IsolationRuntime;
        };

        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.pkg-config
            pkgs.flutter
            pkgs.jdk17
            pkgs.fontconfig
            pkgs.noto-fonts-cjk-sans
            pkgs.watch
          ]
          # reqwest uses rustls (no OpenSSL); Darwin still needs libiconv.
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          JAVA_HOME = "${pkgs.jdk17.home}";

          shellHook = ''
            echo "TempestMiku dev shell · $(rustc --version)"
          '';
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    ))
    // {
      nixosModules.m4Worker = import ./nix/m4-worker-module.nix;
    };
}
