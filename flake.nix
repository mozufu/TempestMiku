{
  description = "TempestMiku — code-execution agent runtime and Flutter client";

  inputs = {
    # Flutter on 25.05 carries Dart 3.7+, required by the pinned offline QR scanner.
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
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
    flake-utils.lib.eachDefaultSystem (
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
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            [
              rustToolchain
              pkgs.pkg-config
              pkgs.flutter
              pkgs.jdk17
              pkgs.fontconfig
              pkgs.noto-fonts-cjk-sans
            ]
            # reqwest uses rustls (no OpenSSL); darwin still wants libiconv and these
            # frameworks for linking network-touching crates.
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
              pkgs.darwin.apple_sdk.frameworks.Security
              pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          JAVA_HOME = "${pkgs.jdk17.home}";

          shellHook = ''
            echo "TempestMiku dev shell · $(rustc --version)"
          '';
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}
