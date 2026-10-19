{
  description = "mesh vst: seeded sampler instrument (CLAP, VST3)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ (import rust-overlay) ]; };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          targets = [ "x86_64-pc-windows-gnu" ];
        };
        # Plugin GUI (baseview, wgpu) and the standalone (JACK, ALSA).
        libs = with pkgs; [
          alsa-lib jack2 libGL vulkan-loader
          libx11 libxcursor libxrandr libxi libxcb libxcb-wm
          libxkbcommon wayland
        ];
      in {
        devShells.default = pkgs.mkShell {
          packages = [ rust pkgs.pkg-config pkgs.python3 ] ++ libs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
          RUST_BACKTRACE = "1";
        };
      });
}
