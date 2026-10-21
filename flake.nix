{
  # The same inputs as mic-rtsim and Deep Time (their flake.lock is copied here), so all three share
  # one nixpkgs and one Rust toolchain in the store.
  description = "mic-viewer: the shared world viewer of mic-rtsim and Deep Time";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    systems.url = "github:nix-systems/default";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import inputs.systems;
      perSystem = { pkgs, lib, system, ... }: let
        # The one Rust version. rust-toolchain.toml must name the same channel;
        # checks.toolchain-channel fails `nix flake check` when they drift.
        rustChannel = "1.98.1";
        rust = pkgs.rust-bin.stable.${rustChannel}.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" ];
        };
      in {
        _module.args.pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [ inputs.rust-overlay.overlays.default ];
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            rust
            pkgs.git
            # The page checks: node runs the pure-function tests of web/, Chromium and Python the
            # browser smoke test and the screenshots (tools/smoke.py).
            pkgs.nodejs
            pkgs.python3
          ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.chromium pkgs.dejavu_fonts ];
          env.RUST_SRC_PATH = "${rust}/lib/rustlib/src/rust/library";
        };

        checks.toolchain-channel = let
          toml = builtins.fromTOML (builtins.readFile ./rust-toolchain.toml);
        in
          assert lib.assertMsg (toml.toolchain.channel == rustChannel)
            "rust-toolchain.toml channel ${toml.toolchain.channel} != flake rustChannel ${rustChannel}";
          pkgs.runCommand "toolchain-channel-ok" { } "touch $out";
      };
    };
}
