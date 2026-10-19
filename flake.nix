{
  description = "mesh vst: seeded sampler instrument (CLAP, VST3)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    # The CLAP conformance tester (not in nixpkgs).
    clap-validator-src = {
      url = "github:free-audio/clap-validator";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, clap-validator-src, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ (import rust-overlay) ]; };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          targets = [ "x86_64-pc-windows-gnu" ];
        };
        craneLib = (crane.mkLib pkgs).overrideToolchain rust;

        # Plugin GUI (baseview, wgpu) and the standalone (JACK, ALSA).
        libs = with pkgs; [
          alsa-lib jack2 libGL vulkan-loader
          libx11 libxcursor libxrandr libxi libxcb libxcb-wm
          libxkbcommon wayland
        ];

        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type: (craneLib.filterCargoSources path type) || (builtins.match ".*bundler\\.toml$" path != null);
        };
        common = {
          inherit src;
          strictDeps = true;
          pname = "mesh-vst";
          version = "0.1.0";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = libs;
        };
        cargoArtifacts = craneLib.buildDepsOnly common;

        # The CLAP and VST3 bundles, built by the nice-plug bundler.
        plugin = craneLib.mkCargoDerivation (common // {
          inherit cargoArtifacts;
          doInstallCargoArtifacts = false;
          buildPhaseCargoCommand = "cargo xtask bundle mesh-vst --release";
          installPhaseCommand = ''
            mkdir -p $out/lib/clap $out/lib/vst3
            cp -r "target/bundled/mesh vst.clap" $out/lib/clap/
            cp -r "target/bundled/mesh vst.vst3" $out/lib/vst3/
          '';
        });

        hostcheck = craneLib.buildPackage (common // {
          inherit cargoArtifacts;
          pname = "hostcheck";
          cargoExtraArgs = "-p hostcheck";
          doCheck = false;
        });

        clap-validator = craneLib.buildPackage {
          pname = "clap-validator";
          version = "0.4.1";
          src = craneLib.cleanCargoSource clap-validator-src;
          strictDeps = true;
          doCheck = false;
        };
      in {
        packages = {
          default = plugin;
          mesh-vst = plugin;
          inherit hostcheck clap-validator;
        };

        checks = {
          # Debug profile: the RT tests count allocations, which needs debug assertions.
          tests = craneLib.cargoTest (common // {
            CARGO_PROFILE = "dev";
            cargoArtifacts = craneLib.buildDepsOnly (common // { CARGO_PROFILE = "dev"; });
            cargoTestExtraArgs = "-p mesh-vst-core";
          });
          clippy = craneLib.cargoClippy (common // { inherit cargoArtifacts; cargoClippyExtraArgs = "-p mesh-vst-core -p mesh-vst -- -D clippy::correctness"; });
          # The bundle passes the CLAP conformance tests and plays through a host.
          validate = pkgs.runCommand "mesh-vst-validate" { } ''
            ${clap-validator}/bin/clap-validator validate "${plugin}/lib/clap/mesh vst.clap" | sed "s/\x1b\[[0-9;]*m//g" > $out
            tail -1 $out
            grep -q " 0 failed" $out
            ${hostcheck}/bin/hostcheck "${plugin}/lib/clap/mesh vst.clap" --mode drums --seed 7 --bars 2 --out drums.wav >> $out
            ${hostcheck}/bin/hostcheck "${plugin}/lib/clap/mesh vst.clap" --mode bass --seed 7 --bars 2 --out bass.wav >> $out
            if grep -q "NON-FINITE" $out; then exit 1; fi
          '';
        };

        apps = {
          hostcheck = { type = "app"; program = "${hostcheck}/bin/hostcheck"; };
          clap-validator = { type = "app"; program = "${clap-validator}/bin/clap-validator"; };
        };

        devShells.default = craneLib.devShell {
          packages = [ pkgs.pkg-config clap-validator ] ++ libs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
          RUST_BACKTRACE = "1";
        };
      });
}
