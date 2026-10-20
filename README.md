# mesh vst

A seeded sampler instrument (CLAP, VST3) for breakbeat drums and mudpie bass.

Hold one note in your DAW and the plugin plays a pattern generated from a seed, in time with the
host tempo and in the key of the note. Roll seeds until a groove fits, drag samples onto slots,
and automate density, variation and permute for the arrangement.

**Status:** alpha. Every design decision (docs/design.md D1–D39) is built: seeded drum and bass
patterns, the sample library with automatic pools, loudness calibration, unison, the editor with
the point-field visuals, and 16 mod-out channels. Not yet tested in a real DAW. Design and progress: [docs/design.md](docs/design.md).

## Planned

- Two modes per instance: **Drums** (kick, snare, hats, ride, shaker, perc, top, break) and
  **Bass** (mudpie WAVs with filter envelope, LFO, sample scrub, stab drive, glide).
- A sound seed and a pattern seed, each with a dice button, a lock and favorites.
- Deterministic playback: the same seed, parameters and song position always render the same.
- Library folders scanned once and sorted into kit pools automatically.
- Mod wheel, pitch bend, channel pressure and MPE.
- Linux and Windows first, macOS later.

## License

FOSS, pay what you want. GPL-3.0-or-later, see [LICENSE](LICENSE).

## Building

With Nix:

```sh
nix build                # result/lib/clap/mesh vst.clap, result/lib/vst3/mesh vst.vst3
nix build .#windows       # Windows bundles, cross-compiled (mingw-w64, system DLLs only)
nix build .#release       # zips of the Linux and Windows bundles
nix flake check          # unit tests, clippy, clap-validator, end-to-end suite in a headless host
nix run .#e2e -- ~/Music/samples   # the end-to-end suite against a real library (read only)
nix run .#screenshot -- editor.png   # the editor on a virtual X server, playing a demo note
```

Inside the devshell (`nix develop`):

```sh
cargo xtask bundle mesh-vst --release   # target/bundled/mesh vst.clap and .vst3
cargo test -p mesh-vst-core             # generator, sampler and player tests
```

Copy the bundles to `~/.clap` and `~/.vst3` (Linux) or `C:\Program Files\Common Files\CLAP` and
`C:\Program Files\Common Files\VST3` (Windows). The Linux bundles are built by Nix and link against
its libraries; on other distributions they need a glibc at least as new as nixpkgs'.

Single renders without a DAW:

```sh
nix run .#hostcheck -- render "result/lib/clap/mesh vst.clap" --mode bass --seed 7 --bars 8 --out bass.wav
cargo run -p mesh-vst-core --release --example render -- --mode drums --kick k.wav --snare s.wav --break amen.wav --out drums.wav
```

## Trying it without a DAW, or with a FOSS one

```sh
nix run .#play              # the standalone editor with sound; the VMPK keyboard window plays it
MESH_VST_DEMO=1 nix run .#play   # the same, holding a note from the start
nix run .#qtractor          # Qtractor (own config) with this build on its CLAP and VST3 paths
```

In Qtractor: Track > Add Track > MIDI, then the track's Plugins tab > Add > "mesh vst", then draw one long note in the clip.

`nix develop` prints every run, test and build command.

## Library

Set the folders in `~/.config/mesh-vst/settings.json`:

```json
{ "library_folders": ["/home/me/Music/Samples"] }
```

(or the `MESH_VST_LIBRARY` environment variable, separated like `PATH`). The first load scans the
folders, later loads read the cache in `~/.cache/mesh-vst/`. Without a library the plugin plays
built-in synthesized sounds.

## Playing it

Put one long note on the track. The note's pitch is the root (bass), its velocity scales the
hits. The pattern follows the song position, so loops and bounces repeat exactly. Set **Mode**
(drums or bass), **Sound seed** and **Pattern seed**, then play with the eight knobs: Density,
Permute, Variation, Swing, Shape, Morph, Spread and Slide. Each knob's middle is the seed's
own suggestion (Permute: 0 plays the pattern as generated). The mod wheel pushes Permute, pitch
bend bends by **Bend range**. A new instance starts with random seeds and knobs.
