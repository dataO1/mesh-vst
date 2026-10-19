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
          filter = path: type: (craneLib.filterCargoSources path type) || (builtins.match ".*bundler\\.toml$" path != null) || (builtins.match ".*/assets/.*" path != null);
        };
        common = {
          inherit src;
          strictDeps = true;
          pname = "mesh-vst";
          version = "0.1.0";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = libs;
        };
        # The dependency build keeps the real vendored crates (crane stubs
        # local crates out otherwise).
        dummySrc = craneLib.mkDummySrc {
          inherit src;
          extraDummyScript = ''
            rm -rf $out/vendor
            cp -r ${./vendor} $out/vendor
            chmod -R u+w $out/vendor
          '';
        };
        cargoArtifacts = craneLib.buildDepsOnly (common // { inherit dummySrc; });

        # The CLAP and VST3 bundles, built by the nice-plug bundler.
        plugin = craneLib.mkCargoDerivation (common // {
          inherit cargoArtifacts;
          CARGO_PROFILE_RELEASE_DEBUG = "0";
          CARGO_PROFILE_RELEASE_STRIP = "symbols";
          doInstallCargoArtifacts = false;
          buildPhaseCargoCommand = "cargo xtask bundle mesh-vst --release";
          installPhaseCommand = ''
            mkdir -p $out/lib/clap $out/lib/vst3
            cp -r "target/bundled/mesh vst.clap" $out/lib/clap/
            cp -r "target/bundled/mesh vst.vst3" $out/lib/vst3/
          '';
        });

        # The standalone build, for the editor screenshot.
        standalone = craneLib.buildPackage (common // {
          inherit cargoArtifacts;
          pname = "mesh-vst-standalone";
          cargoExtraArgs = "-p mesh-vst --bin mesh-vst";
          doCheck = false;
        });

        # Run the standalone editor on a virtual X server and capture it.
        # Inside the virtual X server: start the editor, wait, capture.
        capture = pkgs.writeShellScript "mesh-vst-capture" ''
          MESH_VST_DEMO=1 ${standalone}/bin/mesh-vst --backend dummy >"$1.log" 2>&1 &
          pid=$!
          sleep 6
          ${pkgs.imagemagick}/bin/import -window root "$1"
          kill $pid || true
        '';
        screenshot = pkgs.writeShellScript "mesh-vst-screenshot" ''
          set -eu
          out="$(realpath -m "''${1:-mesh-vst.png}")"
          export MESH_VST_DEMO_MODE="''${2:-drums}"
          export MESH_VST_DEMO_VIEW="''${3:-}"
          export HOME="$(mktemp -d)" XDG_CACHE_HOME="$(mktemp -d)" XDG_CONFIG_HOME="$(mktemp -d)"
          export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (libs ++ [ pkgs.mesa ])}"
          export LIBGL_ALWAYS_SOFTWARE=1
          ${pkgs.xvfb-run}/bin/xvfb-run -a -s "-screen 0 1280x800x24" ${capture} "$out"
          echo "$out"
        '';

        # Windows bundles, cross-compiled with mingw-w64.
        mingw = pkgs.pkgsCross.mingwW64;
        windows = craneLib.mkCargoDerivation {
          inherit src dummySrc;
          pname = "mesh-vst-windows";
          version = "0.1.0";
          CARGO_PROFILE_RELEASE_DEBUG = "0";
          CARGO_PROFILE_RELEASE_STRIP = "symbols";
          strictDeps = true;
          doCheck = false;
          cargoArtifacts = null;
          doInstallCargoArtifacts = false;
          depsBuildBuild = [ mingw.stdenv.cc ];
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${mingw.stdenv.cc}/bin/${mingw.stdenv.cc.targetPrefix}cc";
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${mingw.windows.pthreads}/lib";
          buildPhaseCargoCommand = "cargo xtask bundle mesh-vst --release --target x86_64-pc-windows-gnu";
          installPhaseCommand = ''
            mkdir -p $out/clap $out/vst3
            cp -r "target/bundled/mesh vst.clap" $out/clap/
            cp -r "target/bundled/mesh vst.vst3" $out/vst3/
          '';
        };

        # Release archives: CLAP and VST3 bundles per platform.
        release = pkgs.runCommand "mesh-vst-release" { nativeBuildInputs = [ pkgs.zip ]; } ''
          mkdir -p $out work/linux work/windows
          cp -r "${plugin}/lib/clap/mesh vst.clap" "${plugin}/lib/vst3/mesh vst.vst3" work/linux/
          cp -r "${windows}/clap/mesh vst.clap" "${windows}/vst3/mesh vst.vst3" work/windows/
          chmod -R u+w work
          (cd work/linux && zip -qr $out/mesh-vst-linux-x86_64.zip .)
          (cd work/windows && zip -qr $out/mesh-vst-windows-x86_64.zip .)
        '';

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
          inherit hostcheck clap-validator standalone windows release;
        };

        checks = {
          inherit windows;
          # Debug profile: the RT tests count allocations, which needs debug assertions.
          tests = craneLib.cargoTest (common // {
            CARGO_PROFILE = "dev";
            cargoArtifacts = craneLib.buildDepsOnly (common // { inherit dummySrc; CARGO_PROFILE = "dev"; });
            cargoTestExtraArgs = "-p mesh-vst-core";
          });
          clippy = craneLib.cargoClippy (common // { inherit cargoArtifacts; cargoClippyExtraArgs = "-p mesh-vst-core -p mesh-vst -- -D clippy::correctness"; });
          # The bundle passes the CLAP conformance tests.
          validate = pkgs.runCommand "mesh-vst-validate" { } ''
            ${clap-validator}/bin/clap-validator validate "${plugin}/lib/clap/mesh vst.clap" | sed "s/\x1b\[[0-9;]*m//g" > $out
            tail -1 $out
            grep -q " 0 failed" $out
          '';
          # The editor opens and draws (on a virtual X server, software rendering).
          gui = pkgs.runCommand "mesh-vst-gui" { } ''
            ${screenshot} $TMPDIR/editor.png
            mean=$(${pkgs.imagemagick}/bin/magick $TMPDIR/editor.png -format "%[fx:mean]" info:)
            echo "editor screenshot mean $mean" | tee $out
            awk -v m="$mean" 'BEGIN { exit !(m > 0.02) }'
          '';
          # End to end through a CLAP host: every scenario of `hostcheck suite`,
          # with the built-in sounds and a synthesized library.
          e2e = pkgs.runCommand "mesh-vst-e2e" { } ''
            set -o pipefail
            export HOME=$TMPDIR XDG_CACHE_HOME=$TMPDIR/cache
            ${hostcheck}/bin/hostcheck suite "${plugin}/lib/clap/mesh vst.clap" --out $TMPDIR/e2e | tee $out
          '';
        };

        apps = {
          # A screenshot of the editor: nix run .#screenshot -- out.png [drums|bass] [drag|settings|library]
          screenshot = { type = "app"; program = toString screenshot; };
          # The same scenarios against a real library folder (read only):
          #   nix run .#e2e -- ~/Music/samples [out dir]
          e2e = {
            type = "app";
            program = toString (pkgs.writeShellScript "mesh-vst-e2e" ''
              if [ $# -lt 1 ]; then echo "usage: nix run .#e2e -- <library folder> [out dir]"; exit 2; fi
              out="''${2:-./e2e-out}"
              export XDG_CACHE_HOME="$(mktemp -d)"
              exec ${hostcheck}/bin/hostcheck suite "${plugin}/lib/clap/mesh vst.clap" --library "$1" --out "$out"
            '');
          };
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
