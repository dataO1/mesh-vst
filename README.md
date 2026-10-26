<div align="center">

```
M E S H // V S T
```

**an abstract pattern machine for drums and bass**

`seed` · `roll` · `listen` · `keep`

</div>

---

mesh is controlled chaos. It finds patterns and arranges them for you: you set how wild,
the seed decides the details. Hold a note, and a seed turns into a groove: breakbeat drums or a
bass line cut from a mudpie of your own samples, in time and in key. Roll again and something
else appears. Most rolls you skip. Some you keep and build a track on.

The chaos has rules. Every pattern answers another, every arrangement follows a form, and the
same seed always plays the same thing. A handful of knobs each move many things at once, so you
steer the whole instead of placing notes.

<p align="center"><img src="docs/img/drums.png" alt="drums" width="820"></p>

```
// drums     kick, snare, hats, cymbals, percussion and a chopped break
             style · shape · morph · tone · heat

// bass      one mudpie sample, scrubbed, pitched and filtered into a line
             shape · morph · spread · slide · heat · neuro

// both      density · contrast · variation · groove · squash
             seeds for the sound, the pattern, the phrases, the fills and the second pass
```

<p align="center"><img src="docs/img/bass.png" alt="bass" width="820"></p>

### chaos in the patterns

- **found, not written.** Each letter (A, B, C, D) is a pattern the seed finds: a groove from the
  drum styles, a riff from your MIDI clips or the generator. Roll a letter, pin the ones you like.
- **question and answer.** A asks, B answers. Contrast sets how far apart they sit: at full,
  stabby against sustained, busy against sparse, another drum style.
- **edit when you want to.** The EDIT view shows any pattern's whole source: move notes, pick the
  window that loops, snap to the grid. Everything after it (Density, Groove, the operators)
  still plays on top.

### chaos in the arrangement

- **cycles and motives.** A and B repeat as a cycle (A+B or A+B+A, the seed picks the split). A
  motive repeats its cycle over 16, 32 or 64 bars; the second motive plays the letters' other
  patterns. Fills mark the seams, and the line turns around before them.
- **a 128-bar form.** Two passes of 64 bars, the second a variation of the first. Variation sets
  how much changes.
- **or arrange in your DAW.** Switch FORM to EXT and play the eight patterns (A1 .. D2) from
  keys 36 to 43; a note's velocity picks its operator. `.MID ↓` writes the plugin's own form as
  a MIDI file to start from.

### the idea

- **serendipity first.** Every knob's middle is the seed's own suggestion. You steer it, the
  seed decides the details.
- **limits are the point.** Few controls leave more room to listen.
- **same seed, same groove.** Everything is deterministic, so loops, bounces and recalls play
  exactly what you heard. Every parameter is automatable, and undo covers all of it.
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
