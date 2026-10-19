# mesh vst

A seeded sampler instrument (CLAP, VST3) for breakbeat drums and mudpie bass.

Hold one note in your DAW and the plugin plays a pattern generated from a seed, in time with the
host tempo and in the key of the note. Roll seeds until a groove fits, drag samples onto slots,
and automate intensity and density for the arrangement.

**Status:** early. The CLAP and VST3 plugin plays a seeded drum or bass pattern with built-in
sounds; the library, GUI and visuals are next. Design and progress: [docs/design.md](docs/design.md).

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

Inside the Nix devshell (`nix develop`):

```sh
cargo xtask bundle mesh-vst --release   # target/bundled/mesh vst.clap and .vst3
cargo test -p mesh-vst-core             # generator, sampler and player tests
```

Copy the bundles to `~/.clap` and `~/.vst3` (Linux) or `%COMMONPROGRAMFILES%\CLAP` and
`%COMMONPROGRAMFILES%\VST3` (Windows).

Checks without a DAW:

```sh
cargo run -p hostcheck --release -- "target/bundled/mesh vst.clap" --mode bass --seed 7 --bars 8 --out bass.wav
cargo run -p mesh-vst-core --release --example render -- --mode drums --kick k.wav --snare s.wav --break amen.wav --out drums.wav
```

## Playing it

Put one long note on the track. The note's pitch is the root (bass), its velocity scales the
hits. The pattern follows the song position, so loops and bounces repeat exactly. Set **Mode**
(drums or bass), **Sound seed** and **Pattern seed**, then play with the eight knobs: Density,
Intensity, Variation, Swing, Shape, Morph, Spread and Slide. Each knob's middle is the seed's
own suggestion. The mod wheel pushes Intensity, pitch bend bends by **Bend range**.
