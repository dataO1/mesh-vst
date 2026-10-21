<div align="center">

```
M E S H // V S T
```

**an abstract pattern machine for drums and bass**

`seed` · `roll` · `listen` · `keep`

</div>

---

mesh grows patterns out of numbers. Hold a note, and a seed turns into a groove: breakbeat
drums or a bass line cut from a mudpie of your own samples, in time and in key. Roll again and
something else appears. Most rolls you skip. Some you keep and build a track on.

It is made to be quick to start with and slow to finish exploring. There is no piano roll and
no step editor, only a handful of knobs that each move many things at once. Fewer choices, more
surprises.

<p align="center"><img src="docs/img/drums.png" alt="drums" width="820"></p>

```
// drums     kick, snare, hats, cymbals, percussion and a chopped break
             style · shape · morph · tone · heat

// bass      one mudpie sample, scrubbed, pitched and filtered into a line
             shape · morph · spread · slide · heat

// both      density · permute · variation · groove
             two seeds, one for the sound and one for the pattern
             a form that turns the bars into A B C D blocks
```

<p align="center"><img src="docs/img/bass.png" alt="bass" width="820"></p>

### the idea

- **serendipity first.** Every knob's middle is the seed's own suggestion. You steer it, the
  seed decides the details.
- **limits are the point.** No piano roll and no grids to fill in. Fewer controls leave more
  room to listen.
- **same seed, same groove.** Everything is deterministic, so loops, bounces and recalls play
  exactly what you heard.
- **your sounds.** Point it at your sample folders. It sorts them into kits by itself, and you
  can drop files onto any slot.

Made for producers of breakbeat, jungle, dnb, bass music and anything stranger. Free and open
source (GPL-3.0), no accounts, no upsell.

---

### get it

**[→ releases](https://github.com/dataO1/mesh-vst/releases)** · CLAP and VST3 · Linux and Windows

1. Unzip the release.
2. Copy `mesh vst.clap` and `mesh vst.vst3` to your plugin folders:
   - Linux: `~/.clap` and `~/.vst3`
   - Windows: `C:\Program Files\Common Files\CLAP` and `...\VST3`
3. Load it on an instrument track, hold one long note, and start rolling.
4. Optional: open **SET** in the plugin and add your sample folders.
5. Optional: put your own bass riffs as MIDI files in `~/.config/mesh-vst/clips/a`, `b`, `c`
   and `d` (one folder per letter, any key and length). The bass plays them when its SOURCE
   is CLIPS, the default.

<sub>building from source, tests and dev notes: [docs/building.md](docs/building.md) · design: [docs/design.md](docs/design.md) · music theory: [docs/music-theory.md](docs/music-theory.md)</sub>
