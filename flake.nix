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
    let
      mkPackages = pkgs:
        let
          pkgs' = pkgs.extend (import rust-overlay);

          rustToolchain = pkgs'.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "clippy" ];
          };

          craneLib = (crane.mkLib pkgs').overrideToolchain (_: rustToolchain);

          src = pkgs'.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              (craneLib.filterCargoSources path type)
              || (builtins.match ".*\.html$" path != null)
              || (builtins.match ".*\.json$" path != null)
              || (builtins.match ".*/static/.*" path != null);
          };

          commonBuildInputs = with pkgs'; [
            openssl
            libiconv
          ];

          commonNativeBuildInputs = with pkgs'; [
            pkg-config
            makeWrapper
          ];

          commonArgs = {
            inherit src;
            strictDeps = true;
            buildInputs = commonBuildInputs;
            nativeBuildInputs = commonNativeBuildInputs;
            version = "0.1.0";
          };

          cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
            pname = "train-server-deps";
            cargoExtraArgs = "--locked --workspace";
          });

          train-server = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "train-server";

            TRAIN_SERVER_GIT_HASH = if (self ? rev) && (self.rev != null) then self.rev else "dirty";

            cargoExtraArgs = "--locked -p train-server";

            postInstall = ''
              mkdir -p $out/share/train-server
              cp -r train-server/static $out/share/train-server/
              cp -r train-server/templates $out/share/train-server/

              wrapProgram $out/bin/train-server \
                --set-default STATIC_DIR "$out/share/train-server/static"
            '';

            meta = with pkgs'.lib; {
              description = "Train planner server";
              homepage = "https://github.com/Smaug123/train-planner";
              license = licenses.mit;
              maintainers = [ ];
            };
          });
        in
        {
          default = train-server;
          train-server = train-server;
        };
    in
    {
      lib.mkPackages = mkPackages;
    }
    // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
        pkgs' = pkgs.extend (import rust-overlay);
        rustToolchain = pkgs'.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "clippy" ];
        };
        craneLib = (crane.mkLib pkgs').overrideToolchain (_: rustToolchain);
      in
      {
        packages = mkPackages pkgs;

        devShells.default = craneLib.devShell {
          packages = with pkgs'; [
            pkg-config
            openssl
            libiconv
            claude-code
            codex
          ];

          RUST_BACKTRACE = "1";
        };
      });
}
