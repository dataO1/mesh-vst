# mesh vst

A seeded sampler instrument (CLAP, VST3) for breakbeat drums and mudpie bass.

Hold one note in your DAW and the plugin plays a pattern generated from a seed, in time with the
host tempo and in the key of the note. Roll seeds until a groove fits, drag samples onto slots,
and automate intensity and density for the arrangement.

**Status:** design phase. Nothing is built yet. See [docs/design.md](docs/design.md).

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
