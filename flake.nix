{
  description = "Train journey planner";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config.allowUnfree = true;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "clippy" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (craneLib.filterCargoSources path type)
            || (builtins.match ".*\.html$" path != null)
            || (builtins.match ".*\.json$" path != null);
        };

        commonBuildInputs = with pkgs; [
          openssl
          libiconv
        ];

        commonNativeBuildInputs = with pkgs; [
          pkg-config
        ];

        commonArgs = {
          inherit src;
          strictDeps = true;
          buildInputs = commonBuildInputs;
          nativeBuildInputs = commonNativeBuildInputs;
          version = "0.1.0";
        };

        # Build *only* the dependencies - this derivation gets cached
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
          pname = "train-server-deps";
          cargoExtraArgs = "--locked --workspace";
        });

        train-server = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "train-server";

          # Pass git revision to the build
          TRAIN_SERVER_GIT_HASH = if (self ? rev) && (self.rev != null) then self.rev else "dirty";

          # Build only the server binary
          cargoExtraArgs = "--locked -p train-server";

          meta = with pkgs.lib; {
            description = "Train planner server";
            homepage = "https://github.com/Smaug123/train-planner";
            license = licenses.mit;
            maintainers = [ ];
          };
        });
      in
      {
        packages = {
          default = train-server;
          train-server = train-server;
        };

        devShells.default = craneLib.devShell {
          packages = [
            pkgs.pkg-config
            pkgs.openssl
            pkgs.libiconv
            pkgs.claude-code
            pkgs.codex
          ];

          RUST_BACKTRACE = "1";
        };
      });
}
