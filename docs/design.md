# mesh vst: design

Status: design in progress (grilling rounds 1–3, 2026-10-19). Kept up to date as decisions land.
Each decision has a code (D1 …) so issues and commits can cite it. Open items are under
[Open questions](#open-questions) and [Research](#research).

## 1. Goal

A seeded sampler instrument for production in a DAW. One plugin instance is one lane: a drum
kit or a bass/mid-bass mudpie. Patterns are generated from a seed. The user tries seeds, swaps
samples by drag and drop, and automates a small set of parameters in the DAW. The arrangement
(intro, build, drop) is the DAW's job: note placement and automation.

The pattern generation and sampler voice start from Mesh live mode (`mesh-core/src/live/generator`
and `live/sampler`, about 13k lines, depending only on a key type and a stereo sample type).
The code is copied, not shared: this project is independent of Mesh.

Out of scope: an effects chain inside the plugin (EQ, compression, reverb), MIDI out, an
arrangement engine, ML models.

## 2. Decisions

### Platform

- **D1** One plugin with a mode switch at the top: **Drums** or **Bass**. The mode is picked
  when the instance is set up (drum rack vs synth lane), not switched while playing. Each mode
  shows its own controls.
- **D2** Formats: CLAP and VST3. Platforms: Linux and Windows first, macOS later (signing,
  notarization). No AU, so no Logic for now. A standalone JACK build exists for development.
- **D3** FOSS, funded by donations.
- **D4** Framework: [nice-plug](https://codeberg.org/RustAudio/nice-plug) (ISC, the maintained
  NIH-plug successor; VST3 through the MIT `vst3` crate since the VST3 SDK went MIT in Oct 2025).
  GUI: `nice-plug-iced` (iced 0.13 on baseview) with the wgpu renderer for shader visuals.
- **D5** The core (generator, sampler, sample library) has no GUI or plugin-API code. The GUI and
  the plugin wrapper are separate crates, so either can be replaced.

### Playback

- **D6** Deterministic: the audio is a pure function of seed, parameters and song position (bar
  index). Loops, scrubbing and bounces always render the same. No random state carries between
  bars.
- **D7** Note gate. Sound plays only while an incoming MIDI note is held. The usual use is one long
  note over the whole clip. In Bass mode the note pitch is the root. In Drums mode the pitch is
  ignored for now (pitching drums to the key needs drum pitch detection, deferred).
- **D8** Pattern phase, a toggle: **continuous** (default; the pattern is locked to song
  position, so a note from bar 9 plays what bar 9 would play) or **retrigger** (the pattern
  restarts at note-on).
- **D9** Note-off closes the gate on the next 16th step; sounding samples finish their release.
- **D10** Overlapping notes: the last note wins; a new root glides or retriggers per a parameter.
- **D11** Key: the root comes from the note, the scale from a parameter (minor default, phrygian,
  dorian, harmonic minor, major). Reading key and scale from the DAW is being researched (R2);
  where a host provides it, it replaces the parameter.
- **D12** Velocity. The generated pattern sets each hit's velocity, and parameters modify it
  (example: the sparse 8th hits that density adds between quarter and 8th feel come in softer).
  The incoming note velocity scales the whole lane. v1: velocity = loudness. Velocity as a
  different-sounding hit (filter, transient, sample layer) is research item R3.
- **D13** No MIDI out. The DAW bounces audio or keeps the plugin as an instrument.

### Sound

- **D14** Samples are plain audio files (WAV, also FLAC/AIFF). No SF2/SFZ. All modulation that
  lived inside the Mesh mudpie SF2s (filter envelope, LFO, sample-start scrub, vibrato) becomes
  plugin parameters.
- **D15** Bass mode voice: the Mesh sampler voice. Per-note filter envelope, LFO, sample-start
  scrub, stab punch and saturation, mono with glide. Whether filter and drive stay built in or
  move to a linked post chain depends on R1.
- **D16** Drums mode kit slots (from Mesh): kick, snare, hats, ride, shaker, perc, top, break.
  A seed starts with break or perc, not both, and at most one cymbal-type voice (as in Mesh
  live mode now).

### Library

- **D17** Library folders are a per-machine plugin setting (no DAW tells a plugin where its
  samples are). A scan sorts every usable file into pools (kick, snare, hat, cymbal, perc, break,
  fx, mudpie) deterministically, no ML, by filename keywords first and audio features otherwise.
  The scan is cached by path, size and modification time. Feasibility is the open spike S1.
- **D18** Project state stores each used sample's path relative to its library folder plus a
  content hash. A missing file is found again by hash. Samples are not embedded in the project.
- **D19** A sample dropped onto a slot from outside the library is added to the library
  permanently (see Q5 for how).

### Seeds and parameters

- **D20** Two seeds. The **sound seed** picks samples from the pools (or the mudpie in Bass mode).
  The **pattern seed** picks the groove. Each has a dice button and a lock.
- **D21** Every seeded choice can be overridden by hand. Drums: roll the whole kit, roll one slot,
  or drop a file onto a slot. Bass: drop a mudpie or roll one.
- **D22** Favorite seeds can be saved and recalled.
- **D23** The parameter list is fixed and well named, grouped as Shared, Drums and Bass. The
  inactive mode's group does nothing and is hidden. DAW automation stays valid.
- **D24** Pitch bend bends all voices. Its range is editable, with a default set by mode and seed
  from musical steps (1, 3, 5, 7, 12, 19, 24 semitones). Mod wheel drives an Intensity macro
  (filter depth, drive, density together). Channel pressure and MPE per-note pressure drive the
  drive amount; MPE per-note pitch acts as pitch bend. Other CCs go through the DAW's own MIDI
  learn.
- **D25** Energy over time comes from the DAW: automate Intensity and Density, place notes for
  sections. The plugin adds a phrase length (4/8/16 bars, fills at phrase ends counted from song
  position) and the Mesh **form**: letter seeds A/B (main) and C/D (fillers) placed by a form
  string such as `A15 C1`. The form is a parameter with the Mesh default forms as presets, and
  rolling the pattern seed rolls the form too.
- **D26** History: sample loads, mode switches and parameter changes made in the plugin UI
  (recorded on mouse release, not every tick) go on an undo/redo stack, stepped from the top bar.

## 3. Research

- **S1** Sample classification spike (done 2026-10-19). Ran on the 1288 WAVs in
  `~/Music/mesh-live-dev/samples` with the top folder name hidden. No ML.
  - **Speed is not a problem.** The full scan takes 5 s on one thread, 3.9 ms per file. Decoding
    only the first 1 s costs 0.5 points of accuracy.
  - **Filenames are the main signal.** Filename keywords alone get 90.5% right, and 97% of their
    answers are correct.
  - **Audio features are a fallback.** Hand-written rules get 70% at best, with thresholds tuned on
    this same library.
  - **Combined result.** Name first, then features, with an "unsure" answer when the two disagree:
    81.5% right, 15.6% unsure, 96.5% of answers correct.
  - **Per pool.** Kick, snare and loop are reliable (94–99%). Hat and cymbal confuse each other
    through open hats and shakers. Perc (64%) and fx (41%) are not reliable. Perc folders often
    hold loops and hats.
  - **Robustness.** 0.8% of the files have WAV headers that claim more data than they hold, so
    the decoder must tolerate them.
  - **Consequence for D17.** The scan is viable, but the pools it builds are only suggestions.
    The UI needs:
    - an "unsorted" bin for unsure files;
    - a one-click reassign, saved by path;
    - an audition browser.

    Merging hat and cymbal into one "top" pool, and perc, fx and tonal into "other", would remove
    most errors. The thresholds still need a second, differently named library to validate them.
    Spike code: scratchpad `classify-spike/` (Rust, hound + realfft), to be moved into
    `spikes/` here.
- **R1** Post-processing chain (done 2026-10-19). No plugin format lets one plugin drive another
  plugin's parameters.
  - **The plugin formats.** CLAP's `remote-controls` and `param-indication` only run between a
    plugin and its host. VST3 has nothing for this. The CLAP `cv` draft was removed in 2024.
  - **Hosts.**
    - Reaper is the only host where the plugin can create the links itself: it finds the
      effects after it through the Reaper API and writes their parameter links.
    - Bitwig, Cubase 14+ and FL need one manual mapping per knob, through a control-audio output
      or modulators.
    - Live does not pass MIDI from an instrument to the effects after it.
    - Logic is not possible.
  - **Hosting plugins inside mesh vst.** This is possible for CLAP by reusing Mesh's clack host
    (about 4.4k lines).
    - On Linux, the hosted plugins' GUIs need timer and fd plumbing that nice-plug does not
      offer.
    - Wayland embedding is unsolved.
    - A crashing hosted plugin takes the DAW session down.
    - Their latency and state become ours, so a project breaks on a machine missing those
      plugins.
    - VST3 hosting would mean writing a whole host layer.
  - **Ranked by effort and coverage.**
    1. Built-in fixed processing: low to moderate effort, works the same in every host, and
       changes land per note at the exact sample.
    2. Hosted CLAP chain with a parameter contract: high effort, with crash and GUI risk.
    3. Host routing: automatic in Reaper only, manual in the others.
- **R2** DAW key and scale (done 2026-10-19). None of the eight hosts checked sends its project
  key to a third-party VST3 or CLAP plugin through a documented channel. CLAP has no key
  extension, not even a draft.
  - **Reaper.** The plugin can read the MIDI take or editor scale through the Reaper API. This
    is a small effort, but nice-plug must be patched to expose host extensions.
  - **Cubase.** It may fill the VST3 chord and scale fields. Unconfirmed: a logging test plugin
    would settle it.
  - **Live 12.** Only through a companion Max for Live device that writes the song scale into
    mesh vst's parameters.
  - **No route.** Bitwig 6, Studio One / Fender Studio Pro, Logic, FL and Reason.
  - **ARA 2.** It carries key signatures, but it is built for audio regions and is not usable
    for an instrument.
  - Commercial tools (Captain, Scaler) set or detect the key themselves.
  - **Consequence.** D11 stands: the root comes from the held note and the scale from a
    parameter.
- **R3** Velocity timbre: how to make velocity sound like a different hit (filter tracking,
  transient shaping, round robin, layer crossfade) from one sample. Not started.

## 4. Open questions

- **Q1** License: Mesh is AGPL-3.0-or-later. Copied Mesh code must stay AGPL unless the copyright
  holder relicenses it. GPL-3 or AGPL for mesh vst?
- **Q2** History scope: is the undo stack saved with the project, or per session only?
- **Q3** Favorites scope: per machine across projects, per mode? Do they store only the seeds, or
  seeds plus parameters (a preset)?
- **Q4** Presets: is there a preset browser separate from favorites?
- **Q5** Adding a dropped file to the library: copy it into a managed "mesh vst imports" folder,
  or record its path where it is?
- **Q6** Drum pitching to key (deferred from D7): later version or never?
- **Q7** UI layout per mode, top bar contents, visualizer.
- **Q8** Polyphony in Drums mode, choke groups (open/closed hat), voice limits.
- **Q9** Break slicing: how loops are chopped and gated to the density grid in a DAW context
  (Mesh's loop gating, tempo-matched to the host).
- **Q10** Pools after S1: keep 8 fine pools, or use coarse pools (kick, snare, top = hat+cymbal,
  loop, other = perc+fx+tonal) with the fine label as a sorting hint?
- **Q11** Post-processing (after R1). D15 and the "no effects chain" scope line depend on it.
  Options: built-in fixed processing (filter, drive, transient), a hosted CLAP chain with a
  parameter contract, or host routing.
- **Q12** DAW key integrations (after R2): add Reaper's scale read, and a Cubase test, after v1?
  Or keep the scale parameter only?
