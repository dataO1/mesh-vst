# mesh vst: design

Status: design in progress (grilling rounds 1–5, 2026-10-19). Kept up to date as decisions land.
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
  from its analysis and time-stretch dependencies, which mesh vst does not use. License: GPL-3.0-or-later.
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
  note over the whole clip. In Bass mode the note is the root, octave included (2026-10-19):
  notes E1..D#2 (MIDI 28 to 39) play the home register, each octave up or down moves the line
  an octave. In Drums mode the pitch is
  ignored in v1. v2: tonal drums (kick, toms, perc, not hats or cymbals) are pitched to the
  nearest note of the key, using a pitch detected at scan time.
- **D8** Pattern phase, a toggle: **continuous** (default; the pattern is locked to song
  position, so a note from bar 9 plays what bar 9 would play) or **retrigger** (the pattern
  restarts exactly at the note-on, and the round robins start over, so every press plays the same).
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

- **D20** Two seeds. The **sound seed** picks samples from the pools (or the mudpie in Bass mode),
  and in Bass mode also the mudpie voice: cutoff, resonance, filter and pitch envelopes, LFO,
  amp decay and release, drawn around the designed voice (2026-10-19). The knobs act on top. A
  sound roll in Bass mode keeps the mudpie (it gets pinned) and moves every knob but Permute to
  new values (2026-10-20). Accent strength comes from the sound seed, scaled by Variation. A new
  instance without a saved state starts with random seeds and knobs (Permute off).
  The **pattern seed** picks the groove. Each has a dice button and a lock.
  Loading cuts silence: one-shots start at their onset, mudpies lose every silent stretch, and a
  streamed mudpie note plays at the gain that brings its start position to the sample's overall
  loudness. The lane trim (no cap) brings the lane to its LUFS target.
- **D21** Every seeded choice can be overridden by hand. Drums: roll the whole kit, roll one slot,
  or drop a file onto a slot. Bass: drop a mudpie or roll one.
- **D22** Favorite seeds: per machine, shared across projects, filtered by mode. A favorite stores
  the seeds and a name, not parameters or samples. No presets: the plugin is for exploring, and
  the DAW saves the state.
- **D23** The parameter list is fixed and well named, grouped as Shared, Drums and Bass. The
  inactive mode's group does nothing and is hidden. DAW automation stays valid.
- **D24** Pitch bend bends all voices. Its range is editable, with a default set by mode and seed
  from musical steps (1, 3, 5, 7, 12, 19, 24 semitones). Mod wheel drives Permute (was an Intensity macro)
  (filter depth, drive, density together). Channel pressure and MPE per-note pressure drive the
  drive amount; MPE per-note pitch acts as pitch bend. Other CCs go through the DAW's own MIDI
  learn.
- **D25** Energy over time comes from the DAW: automate Permute, Variation and Density, place notes for
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
- **D39** Drum **Style**: a stepped, automatable parameter. Options: Seed (the pattern seed
  picks), Half time, Two step, Rolling, Jungle, Hardcore. These are the Mesh live archetypes,
  each played exactly, not blended. Morph no longer moves the style. It chops and rewrites
  inside it.
- **D30** Pool fixes: unsure files sit in an unsorted bin, any file moves to another pool in one
  click, and these overrides are saved per machine by path. An audition browser lists each pool.
- **D31** Hat, cymbal and shaker pools. The name decides when it carries a keyword: 97% of
  this library's Cymbals folder does. Otherwise the audio decides, and it only separates
  hats and crashes. Measured with a leave-one-out 3-NN on S1 features, which is optimistic:
  hats 90%, crashes 89%, rides 22%, shakers 13%. So hats keep their own pool. Rides, crashes,
  shakers and other tops share one cymbal pool, and the name label is a tag the seed can prefer
  (for example rides for an 8th ride pattern). The two cymbal slots draw from that pool.
- **D32** Modulation outputs. Up to 16 mod-out channels on aux audio outputs. Each channel
  carries a DC level from one internal value, not just envelopes: any generator or voice
  state. The first four are velocity, accent/stab, amp envelope and filter envelope; the full
  list is to be discussed. The user maps a channel once through the DAW's own follower (R4).
  Live needs one audio track per used channel plus a Max for Live Envelope Follower (Suite or
  the add-on). Studio One has no path. No test spike: it is built to spec, and if it fails in
  practice the mechanism changes. The
  bass voice keeps its own filter and drive, and there are no other built-in effects.
- **D33** UI layout (artifact "mesh vst UI"): a fixed global top bar (logo, mode, key from note,
  host tempo, sound and pattern seed with dice and lock, favorite, undo/redo, library,
  play). It is visually modulated by the hits (chromatic aberration ghosts in red and blue as
  in Mesh live). Shared pattern controls on the left, mode controls on the right, a square
  visual in the middle, and a footer strip. Font: Share Tech Mono, display font Major Mono
  Display, Mesh live palette.
  - **Bass:** the visual is a 100×100 point field in the style of Output Portal. Every point is
    always on. Rows are 100 frequency bins (low at the bottom). Each point moves in x and y
    along a slowly turning flow field. The distance is that bin's level, mapped over a
    volume range in dB, so silence is a still grid and loud bins swirl. The effects follow
    the field: brightness, point size and the red/blue split are set per point by its bin
    level. Additive glow and short trails come on top. A file dropped anywhere on the window
    loads as the mudpie. A full-window overlay shows while dragging, and the bottom-left slot
    shows the result. The footer holds the mudpie slot, the step row with velocity, and the
    mod-out meters.
  - **Drums:** the same point field and behavior as Bass, but each point is a short line, not
    a dot. The field is split into one subgrid per loaded sample (kick, snare, hats, cymbals,
    perc or break), and each subgrid moves with its own sample's spectrum. Movement and
    effects bleed a little into the neighboring subgrids. While a file is dragged, the center
    shows the subgrid layout labeled with each sound type, so the user sees where a drop lands.
    Dropping on a subgrid loads that slot. Dropping elsewhere classifies the file (D17) into
    its pool's slot, and an unsure file goes to the unsorted bin with a message. The footer
    slot strip stays. Slots the seed leaves off are dimmed.
- **D34** Mod-out default sources, each channel reassignable:
  - Bass: 1 velocity, 2 stab, 3 amp env, 4 filter env, 5 LFO, 6 scrub position, 7 glide,
    8 note pitch.
  - Drums: 1 velocity, 2 accent, 3 kick, 4 snare, 5 hats, 6 cymbal, 7 break/perc, 8 fill gate.
  - Both: 9 beat ramp, 10 bar ramp, 11 phrase position, 12 form letter, 13 effective density,
    14 permute, 15 mod wheel, 16 pressure.
- **D35** Library side panel from the top bar: pools with counts, the unsorted bin, audition,
  drag onto a slot, and library folder management. The visual runs at 30 fps or less, pauses
  while hidden, and has a low-power static mode.

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

- **Q18** The full parameter list (draft in section 6).

## 5. Todo after v1

- Enrich the drum styles (D39): more archetypes for creative EDM, none generic. Candidates:
  - techstep
  - neuro rollers
  - UK garage 2-step
  - footwork / juke
  - dubstep half time
  - nu-skool breakbeat
  - bounce / jump-up
  - rave hardcore with amen chops
  - minimal dnb

  Each needs kick, snare, hat and ghost level maps in `generator/drums.rs` and a name in
  `DEFAULT_ARCHETYPE_ORDER`. The Style parameter then lists it.
- Slicing breaks on detected onsets (D29).
- Pitching tonal drums to the key (D7).
- Velocity as timbre (R3).
- Generic DAW key read via the VST3 chord and scale context (D11).

## 6. Parameters (D37)

Eight knobs at most: 5 shared and up to 3 per mode, broad like Mesh live's lane controls. All
are automatable. The seed sets each knob's default center, so a new seed comes with its own
suggested settings. Turning a knob or automating it moves away from that center.

| Knob | Bass | Drums |
|---|---|---|
| **Density** | notes, note length | hits, voices coming in (tops included), loop gating |
| **Permute** | half-beat chunks of the bar swap places (seeded, downbeat stays; 0 = as generated) | the same |
| **Variation** | syncopation, rewrites, fills; scales the seeded accents, stab drive and filter sweep | rewrites, fills, ghost notes; scales the seeded accents and velocity range |
| **Swing** | swing | swing |
| **Shape** | stabby to sustained: length, expression, filter bite | tight to open: choke, loop hold, decay |
| **Morph** | mudpie scrub position | straight to chopped (break chop, rewrites) inside the Style |
| **Spread** | octave lifts, jump size | — |
| **Slide** | slides, glide time | — |

A ninth, stepped and automatable parameter, **Style** (drums), picks the rhythm family
(D39).

The **settings panel** holds values that are seeded where sensible, saved with the project, and
not knobs: Rate, Form, Phrase, Phase, Scale, Bend range, Gain, and the mod-out assignment and
meters.

**Unison (Bass only, D38).** Each note plays a seeded number of detuned copies of the mudpie
voice, with a seeded detune and stereo width per note. Some notes come out wide and detuned,
others straight and narrow. The footer's bottom-right widget sets the limits the seed works
within: **Voices** (maximum, 1–8) and **Detune** (maximum, in cents). These two are project
state, not knobs.

The unison sum keeps equal loudness: gain falls with the number of voices (1/√N for the
uncorrelated detuned copies), so a wide note is not louder than a straight one. The **unison
mode** sets how the voices are spread inside the detune range:
- **Linear:** even steps.
- **Soft:** clustered near the center.
- **Hard:** pushed to the edges.
- **Stacked:** octave and fifth layers, each slightly detuned.
- **Random:** fixed seeded offsets.

The mode is a patch setting. The seed does not change it.

## 7. Build order (D36)

Each step is tested before the next one starts.

1. **M1** Core crate: the generator and sampler copied from Mesh, deterministic from song position, with headless render tests.
2. **M2** nice-plug CLAP and VST3 wrapper: note gate, transport, parameters, state, built-in test samples.
3. **M3** Library: scan, cache, pools, hash relink, imports folder.
4. **M4** iced GUI: top bar, parameters, slot strip, drop anywhere, undo history, favorites, library panel.
5. **M5** Visuals: point field (dots for Bass, lines in subgrids for Drums) and the low-power mode.
6. **M6** Mod-out aux channels.
7. **M7** Linux and Windows builds and a release.

## 8. Progress

- **M1 done (2026-10-19).** The `mesh-vst-core` crate holds the Mesh generators (drums, bass,
  forms, layout), the sampler and three new modules:
  - `pattern` renders one bar purely from seeds, form and parameters, without allocating.
  - `player` fires a bar at sample-accurate offsets from the host song position, with the note
    gate, the next-16th close and retrigger phase.
  - `kit` builds the drum and mudpie banks.

  227 tests pass. Output is identical at any block size and allocation-free. The
  `examples/render` tool writes WAVs from real samples.

  Gaps found, all for later milestones:
  - **G1** Break loops play at their own speed. The tempo resampling of D29 needs the loop's
    tempo, from its length or filename (M3), applied as slice pitch in the player.
  - **G2** Samples are not loudness-normalized, so a render can peak above 0 dBFS. Normalize
    at load like Mesh does (M3).
  - **G3** A bass instance does not hear the drums. Mesh's bass followed the drum lane's kicks,
    so its kick-follow behavior is off here.
  - **G4** Break slice types are an amen template until onset slicing exists (todo).
- **M2 done (2026-10-19).** The `mesh-vst` plugin crate (nice-plug 0.4.2) builds CLAP and VST3
  bundles.
  - **Parameters.** The eight knobs are automatable. Mode, seeds, form, phase, scale, rate and
    bend range are non-automatable settings, plus gain.
  - **Knob centers.** The seed sets each knob's middle (`macros`), using the Mesh lane-control
    mappings.
  - **Playing.** A held note gates the pattern. A stopped transport runs a free clock from the
    note-on. Mod wheel adds to Permute, pitch bend bends, and other CCs reach the sampler.
  - **Sounds.** A built-in synthesized kit and mudpie play until the library exists.
  - **Checks.** The CLAP bundle passes clap-validator (36 passed, 0 failed).
    `crates/hostcheck` is a headless clack host that plays a held note against a host
    transport and writes a WAV. Runs repeat byte for byte, seeds differ, a stopped transport
    plays, and a release goes silent after the tails.

  Gaps:
  - **G5** Loudness varies a lot between seeds: drum RMS ranges from −21 to −15 dBFS over five
    seeds.
  - **G6** The choke amount (D27) and unison (D38) are not built yet.
  - **G7** Windows builds need a mingw cross toolchain in the devshell (M7).
- **M3 done (2026-10-19).**
  - **Library module.** `library` handles the scan, the cache keyed by path, size and
    modification time, the classifier ported from S1, pools, tags, overrides, the content hash
    and imports.
  - **Loader module.** `loader` makes the seeded slot picks with pins. It normalizes each
    sample to −12 LUFS and resamples it to the host rate. Break tempo comes from the file name
    or the loop length, and the player follows the host tempo by pitch (G1 fixed). A mudpie's
    root comes from a note name or its detected pitch.
  - **Plugin wiring.** The plugin loads kits on a worker thread and swaps them in without
    allocating. The project saves the files it plays and finds them again by hash (D18).
  - **Settings.** Library folders live in `~/.config/mesh-vst/settings.json` or come from
    `MESH_VST_LIBRARY`.
  - **Accuracy on the dev library.** With folder names hidden: 81.4 % right, 14.9 % unsure,
    95.6 % of answers right, the same as the spike. A full scan of 1288 files takes 6.9 s, a
    cached rescan 0.01 s.
  - **Tests.** They are declarative in the flake:
    - `nix flake check` runs the unit tests (dev profile), clippy, clap-validator, and the e2e
      suite. The suite (`hostcheck suite`) runs 12 scenarios with assertions in a headless CLAP
      host, with the built-in sounds and a synthesized library.
    - `nix run .#e2e -- <folder>` runs the same suite against a real library. Every scenario
      passes on both.
  - **Gaps.**
    - **G8** No UI for pins and overrides yet (M4).
- **Loudness (2026-10-19, fixes G5 and G9).** This works as in Mesh live, in two stages:
  - **Per sample.** Every sample is normalized to −12 LUFS. Drum slots then sit at Mesh's
    sub-voice balance: break −2 dB, hats −8, cymbals and shaker −6, perc −10.
  - **Per lane.** The loader renders 16 bars of the lane with its own generator at the knobs'
    middle, measures BS.1770 loudness and trims the output to −14 LUFS. This reruns when the
    samples, seeds, form or rate change.

  The knobs then change loudness as they should, but a new seed or sample does not. The e2e
  suite checks four seeds per mode, built-in and library, and all land at −14.0 LUFS. Peaks
  above 0 dBFS are left alone: the output is float, and the DAW mix does the gain staging.
- **M4 (2026-10-19, first version).** The iced 0.14 editor runs through nice-plug-iced, on wgpu
  with a tiny-skia fallback, in the Mesh live palette and fonts.
  - **Top bar.** Mode, key from the held note with scale, host tempo, both seeds with roll and
    lock, favorite save and recall, undo/redo, library.
  - **Pattern knobs (left).** The four pattern knobs, phase, form roll, gain.
  - **Mode knobs (right).** Drums: Style, Shape, Morph. Bass: Shape, Morph, Spread, Slide, Rate.
  - **Center.** A live step view of the playing bar. M5 replaces it with the point field.
  - **Footer.** Slot cards with the file, roll and back-to-seed.
  - **Library panel.** Pools with counts, and a file list with a pool chooser per file
    (overrides).
  - **Drag and drop.** A file dropped anywhere is imported, classified and pinned to its pool's
    slot; one dropped on a slot card goes to that card (Q17). Unsure files stay unsorted, with a
    message.
  - **Drop events.** iced_baseview 0.6 does not forward baseview's drop events, so it is vendored
    with a ~40-line patch (`vendor/README.md`). The user accepted this for now. To do: fork and
    offer the patch upstream.
  - **Undo.** Snapshots cover every parameter and the pins (D26).
  - **Favorites.** Stored in `~/.config/mesh-vst/favorites.json`.
  - **Locks.** Saved with the project.
  - **Checks.** `nix run .#screenshot -- out.png` captures the editor on a virtual X server, with
    a demo note held. `checks.gui` asserts that it draws.

- **M5 (2026-10-19, first version).** The point field runs as a wgpu shader through iced's shader
  widget (`visual.rs`).
  - **Drawing.** There are 100 × 100 instanced points in one draw call with three additive
    passes (red, blue, ink). The flow field, displacement, size and color split are computed
    per point on the GPU.
  - **Spectrum.** The editor reads the plugin's output through a lock-free ring while it is open
    and analyses it into 100 log bins (40 Hz–16 kHz, 2048-point FFT, +4.5 dB/oct tilt, 42 dB
    range).
  - **Bass.** Dots, one row per bin.
  - **Drums.** Short lines in 4 × 2 subgrids (kick, snare, hats, ride, shaker, perc, top,
    break). A subgrid lights on its slot's hits with the spectrum as texture and a 25 % bleed to
    its neighbors.
  - **Dropping.** While a file is dragged, the subgrids show labels and the one under the
    cursor is highlighted; a drop on a subgrid loads that slot.
  - **Rate and modes.** Redraws are throttled to about 30 fps. A FIELD / STEPS toggle picks the
    low-power step view, saved with the project.
  - **Screenshots.** `nix run .#screenshot -- out.png [drums|bass] [drag]`.
  - **Not yet.** Trails need a persistent texture. The top bar is not yet modulated by hits.
    Visual sensitivity needs tuning by eye.

- **M6 done (2026-10-19).** The mod outputs (D32, D34) are 16 mono aux outputs next to the stereo
  out, with a stereo-only layout as the fallback. Ports are named by their bass and drum
  source (`Mod 3 amp env / kick`).
  - **Signal.** Every channel is a DC level 0..1, written sample accurately by `core::modout`
    from the fired events and the transport.
  - **Drum sources.** Held velocity, accent, per-voice hit envelopes, fill.
  - **Bass sources.** Held velocity, stab, amp and filter envelopes, LFO, scrub position,
    glide, pitch.
  - **Shared sources.** Beat, bar and 8-bar phrase ramps, form letter, density and permute
    after the seed's centers, mod wheel, channel pressure.
  - **Checks.** The e2e host now provides every declared port and checks that both modes'
    channels stay in range and follow the hits.
  - **Gap.** Reassigning channels to other sources needs a settings UI (G10).

- **M7 (2026-10-19).**
  - **Windows.** Bundles cross-compile with mingw-w64 in the flake (`nix build .#windows`, also a
    check). They import only Windows system DLLs and are stripped to 24 MB.
  - **Release.** `nix build .#release` zips the Linux and Windows CLAP and VST3 bundles.
  - **Untested.** Neither has run in a Windows DAW yet. The Linux bundles need a recent glibc
    outside NixOS.
  - **macOS.** Later (D2).

- **Built after M7 (2026-10-19).**
  - **Unison (D38).** Per-note seeded copies, 1 to 8, with detune and width inside the user's
    Voices and Detune limits. They sum at equal loudness (1/√N, within 2 dB of a plain note
    over 1.5 s). The five modes are linear, soft, hard, stacked (octaves and fifths) and random.
    The bass footer has the widget.
  - **Choke amount (D27).** 1 cuts in 10 ms, 0 lets the voices ring, in between fades over up
    to 0.5 s. It follows the drums' Shape. Mono groups (mudpies) always cut.
  - **Phrase (D25).** 4, 8 or 16 bars. The drums fill the last bar of each phrase, counted from
    song position.
  - **New root (D10).** Glide or retrigger. Retrigger chokes the sounding bass when the held
    note changes its pitch class.
  - **Pressure (D24).** Channel and MPE pressure push the stab drive and the drum accents.
    MPE per-note pitch bends.
  - **Bend range (D24).** A "seed" setting picks it from the mode's musical steps.
  - **Mod-out sources (D34, closes G10).** Every channel can take any of 23 sources. The
    mapping is saved with the project and read lock-free on the audio thread.
  - **Settings panel.** Phrase, new root, bend range, the 16 mod-out sources, and the library
    folders (add, remove, rescan, written to `settings.json`).
  - **Audition (D35).** A play button per file in the library panel; the loader decodes it and a
    preview voice plays it over the lane. The editor never scans: it reads the loader's cache,
    and rescans run on a thread.
  - **Trails (D33).** The field draws into its own texture, which keeps 62 % of the last frame,
    and is added onto the editor.
  - **Top bar (D33).** The logo, the key and the seeds split red/blue with the hits, as in Mesh
    live.
  - **Every decision D1–D39 is built.** D40 (seeded automation) is still undecided. Nothing has
    run in a real DAW yet.

## 9. Research R5: automation in the host (2026-10-19)

No CLAP (1.2.10), VST3 (3.8.1) or ARA API lets a plugin write automation points or clips into a
host's timeline.
- **Recording.** The portable route is recording. The plugin moves its own parameters inside
  begin/end gestures while the track is in write, latch or touch mode. That records as
  editable automation in Live, Bitwig, FL, Cubase and Logic (with gestures), and probably
  Reaper and Studio One. It only works in realtime, at block or GUI rate: long form ramps
  survive, per-hit moves smear.
- **REAPER.** Only REAPER allows direct envelope writes, through its host API: exact curves,
  offline.
- **nice-plug.** It has no audio-thread parameter setter. Plugin-initiated changes go through
  the editor's GuiContext.

**Proposal (D40, not decided):**
- Generate seeded, form-aware parameter movement inside the plugin, sample accurate and visible
  in the UI: exponential ramps into the form's breaking points, moves on hits.
- Add an optional "record to host" mode that sends the movement as gestures while the DAW
  records.
- Stop the internal movement for a parameter once host automation exists, read via CLAP
  param-indication.
