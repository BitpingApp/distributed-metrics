{
  description = "distributed-metrics - Prometheus metrics backed by Bitping's distributed network";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    rust-overlay,
    ...
  }:
  let
    supportedSystems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];
    forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    pkgsFor =
      system:
      import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };
  in
  {
    packages = forAllSystems (
      system:
      let
        pkgs = pkgsFor system;
        rustToolchain = pkgs.rust-bin.stable.latest.default;
      in
      {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "distributed-metrics";
          version = "1.2.3";
          src = ./.;
          cargoHash = "sha256-fEv3zOHDvHNM90ClkwqqTKX1ts+9iOGIsynTmU//+Fc=";
        };
      }
    );

    devShells = forAllSystems (
      system:
      let
        pkgs = pkgsFor system;
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
          ];
        };
      in
      {
        default = pkgs.mkShell {
          buildInputs = [
              rustToolchain
              pkgs.cargo-audit
              pkgs.cargo-watch
              pkgs.cargo-dist
            ];

          env = {
            RUST_BACKTRACE = "1";
          };
        };
      }
    );
  };
}
