# mesh vst: design

Status: design in progress (grilling rounds 1–4, 2026-10-19). Kept up to date as decisions land.
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

Out of scope: mixing effects (EQ, compression, reverb), presets, MIDI notes out, an arrangement
engine, ML models. Built-in per-note processing (filter, drive) is avoided if R4 finds a way to
drive external effects.

## 2. Decisions

### Platform

- **D1** One plugin with a mode switch at the top: **Drums** or **Bass**. The mode is picked
  when the instance is set up (drum rack vs synth lane), not switched while playing. Each mode
  shows its own controls.
- **D2** Formats: CLAP and VST3. Platforms: Linux and Windows first, macOS later (signing,
  notarization). No AU, so no Logic for now. A standalone JACK build exists for development.
- **D3** FOSS, pay what you want. Mainly for personal use. No AGPL needed: the AGPL in Mesh comes
  from its analysis and time-stretch dependencies, which mesh vst does not use. License per Q1.
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
  ignored in v1. v2: tonal drums (kick, toms, perc, not hats or cymbals) are pitched to the
  nearest note of the key, using a pitch detected at scan time.
- **D8** Pattern phase, a toggle: **continuous** (default; the pattern is locked to song
  position, so a note from bar 9 plays what bar 9 would play) or **retrigger** (the pattern
  restarts at note-on).
- **D9** Note-off closes the gate on the next 16th step; sounding samples finish their release.
- **D10** Overlapping notes: the last note wins; a new root glides or retriggers per a parameter.
- **D11** Key: the root comes from the note, the scale from a parameter (minor default, phrygian,
  dorian, harmonic minor, major). After v1: read the key from the DAW only through generic
  plugin-API fields (the VST3 chord and scale context) when a host fills them. No host-specific
  code (R2).
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
  are handed to external effects depends on R4.
- **D16** Drums mode pools and slots: kick, snare, hats, two cymbal slots, perc, break (close to Mesh
  live mode now). A seed starts with break or perc, not both, and at most one cymbal slot
  playing. The hat/cymbal confusion found in S1 is handled by the reassign UI (D30).

### Library

- **D17** Library folders are a per-machine plugin setting (no DAW tells a plugin where its
  samples are). A scan sorts every usable file into pools (kick, snare, hat, cymbal, perc, break,
  mudpie; fx and unsure files go to an unsorted bin) deterministically, no ML, by filename
  keywords first and audio features otherwise.
  The scan is cached by path, size and modification time. S1 showed it is viable (5 s for 1288
  files) but not exact, so pools are suggestions (D30).
- **D18** Project state stores each used sample's path relative to its library folder plus a
  content hash. A missing file is found again by hash. Samples are not embedded in the project.
- **D19** A sample dropped onto a slot from inside a library folder is linked where it is. One
  dropped from outside is copied into `<first library folder>/mesh vst imports/` and scanned.

### Seeds and parameters

- **D20** Two seeds. The **sound seed** picks samples from the pools (or the mudpie in Bass mode).
  The **pattern seed** picks the groove. Each has a dice button and a lock.
- **D21** Every seeded choice can be overridden by hand. Drums: roll the whole kit, roll one slot,
  or drop a file onto a slot. Bass: drop a mudpie or roll one.
- **D22** Favorite seeds: per machine, shared across projects, filtered by mode. A favorite stores
  the seeds and a name, not parameters or samples. No presets: the plugin is for exploring, and
  the DAW saves the state.
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
  The stack lasts for the session and is not saved in the project.
- **D27** Choke: the hats form a choke group, and break slices cut each other. A choke amount
  parameter (0 = no choke, 1 = full cut, in between a shortened release) is set by the seed as
  part of the suggested beat and can be changed or automated.
- **D28** Drums polyphony: a few voices per slot (4 by default). The kick does not choke.
- **D29** Break loops follow the host tempo by resampling (pitch follows speed). Slicing on
  detected onsets is a v2 todo. v1 keeps Mesh's step-grid chops and density gating.
- **D30** Pool fixes: unsure files sit in an unsorted bin, any file moves to another pool in one
  click, and these overrides are saved per machine by path. An audition browser lists each pool.

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
- **R4** Modulation outputs (done 2026-10-19). No mechanism covers every DAW, and no CLAP or
  VST3 standard (as of CLAP 1.2.10 and VST3 3.8.1) defines a modulation output.
  - **Aux control audio output: the widest coverage.** An extra audio output carries the envelope
    as a signal. The user routes it to a follower that modulates the effect, which costs one
    mapping per knob.
    - **Reaper:** audio control signal.
    - **Bitwig:** Audio Sidechain modulator.
    - **Live 12:** Max for Live Envelope Follower (needs Suite or the add-on), with up to 8 maps.
    - **FL:** Peak Controller on the mixer insert, then Link to controller.
    - **Cubase 14+ Pro:** Envelope Follower modulator. Instrument aux outputs as its source are
      not verified.
    - **Reason:** Pulveriser follower into a VST container's CV input.
    - **No path:** Studio One / Fender Studio Pro, and Logic. Logic is out anyway, since there
      is no AU.
    - **nice-plug:** supports aux outputs in both wrappers today.
    - **Caveats:** followers smooth sharp attacks. The aux must not reach the master: DC at the
      speakers.
  - **Output parameters.** Only Reaper can use them as a source. Live needs a custom Max for
    Live device. nice-plug cannot emit them from the audio thread without a fork.
  - **MIDI CC out.** Reliable into effects only in Reaper, likely in Bitwig over CLAP. Live and
    Logic do not route an instrument's MIDI into audio effects. FL and Cubase need the effect's
    own MIDI learn. nice-plug supports it today.
  - **Automation write-back.** Works everywhere, but only offline: record, then copy. It cannot
    follow live per-note state.
  - **Precedent.** Commercial modulation plugins (LFOTool, MidiShaper) use MIDI CC. No known
    product uses an audio control bus.

## 4. Open questions

- **Q1** License: GPL-3.0-or-later (forks stay open) or MIT (anything goes)? The copied Mesh code
  is relicensed by its author.
- **Q7** UI layout per mode, top bar contents, visualizer.
- **Q11** Post-processing after R4: an aux control output (4 envelope channels) as the main path, the
  bass voice keeps its own filter and drive, no further built-in processing?
- **Q13** The two cymbal slots: ride and crash, or two picks from one cymbal pool? Where do shaker
  and top loops go: hats, perc, or break?

## 5. Todo after v1

- Slicing breaks on detected onsets (D29).
- Pitching tonal drums to the key (D7).
- Velocity as timbre (R3).
- Generic DAW key read via the VST3 chord and scale context (D11).
