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
  amp decay and release, drawn around the designed voice (2026-10-19; the release only ±0.3 octave and at most a half note, 2026-10-20). The knobs act on top. A
  sound roll in Bass mode keeps the mudpie (it gets pinned) and moves every knob but Contrast to
  new values (2026-10-20), and rolls the Rate: Normal half the time, Half or Double a quarter
  each (2026-10-20). Accent strength comes from the sound seed, scaled by Variation. A new
  instance without a saved state starts with random seeds and knobs (Permute off).
  The **pattern seed** picks the groove. Each has a dice button and a lock.
  Loading cuts silence: one-shots start at their onset, mudpies lose every silent stretch, and a
  streamed mudpie note plays at the gain that brings its start position to the sample's overall
  loudness. The lane trim (no cap) brings the lane to its LUFS target.
- **D21** Every seeded choice can be overridden by hand. Drums: roll the whole kit, roll one slot,
  or drop a file onto a slot. Bass: drop a mudpie or roll one. A slot's own roll plays until
  the next sound roll; its pin button (top right of the card) keeps it, or a dropped file,
  through sound rolls (2026-10-20).
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
- **D25** Energy over time comes from the DAW: automate Contrast, Variation and Density (Permute too), place notes for
  sections. The plugin adds a phrase length (4/8/16 bars, fills at phrase ends counted from song
  position) and the Mesh **form**: letter seeds A/B (main) and C/D (fillers) placed by a form
  string such as `A15 C1`. The form is generated from three seeds (D48: phrases, fill plan,
  second pass), each rolled on its own; the PATTERN roll rolls the notes and all three (pinned phrases stay). Each
  letter has its own seed and moves the
  parameters around the knobs, the right-hand knobs (Shape, Morph, Spread, Slide), Permute (when
  on) and Groove most; since D49 Contrast, not Variation, sets how far B moves from A, and C
  and D from them; the knobs show each hit's values
  (2026-10-20). Per note only the right-hand knobs move, Shape much (a seeded share), the
  others a little (2026-10-20). Shape's per-note move is small at low Variation and grows on a
  curve to its full reach at Variation 1 (user, 2026-10-21, reversing the earlier "much at low
  Variation too").
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
- **D41** Bass **rhythm cells** (2026-10-20): a seeded share of the bass line's beats is replaced by a
  cell from another grid: 16th and 32nd ratchets, 8th, 16th and quarter triplets, dotted notes,
  held beats into a 16th pickup, a note held over two beats, triplets into a held note. The
  pattern seed picks one family for the whole pattern (plain, straight 16ths, triplets, dotted,
  ratchet clusters, sustained, mixed; plain about 1 in 6). Filler letters (C, D) play another
  family each, with the filler boosts (more variation and spread). Variation sets how many beats
  take a cell. The repeating bars of a phrase pick alike, the variation bar re-draws.
  Rate: double speed plays no faster than 16ths and half speed no faster than 8ths (2026-10-20);
  a note closer to the one before goes, and that one holds over it. Rate's centre is half the
  rendered speed (2026-10-20): Normal plays what half speed did (8ths at the fastest), Double
  what Normal did, Half half of that again. A slowed bar plays its part of the rendered bar,
  or the busiest part when that one is far quieter (stabs count a 16th).
- **D42** **Splice** (2026-10-20): an instance follows a host, another mesh instance in the
  project or the sidechain input, picked in the SPLICE list (right column). The Splice knob runs
  from stacked (0) through off (0.5) to the host's gaps (1). The host shares, by song bar, what it
  plays per 16th; the follower turns it into a focus per step: density, variation and spread
  lean there, hits outside it move a 16th or an 8th into it or thin out (at most 85 % at the
  ends, so the extremes play mostly, not only, stacked or in the gaps). One way only: the list
  hides instances that follow this one. Instances share through a small shared-memory table
  (`/dev/shm/mesh-vst-link-v2` since D46, the temp dir elsewhere), so it works when the host sandboxes
  each plugin; nothing but the bars and band levels crosses, and each follower renders from
  data published a bar ahead, so bounces repeat. The sidechain stands for the next bars with its
  last bar's levels per 16th. Where both lanes are loud in a band, the field draws amber.
- **D43** **Mudpie loops** (2026-10-20): a long note (at least 76 rendered ticks, played at Normal Rate a dotted quarter and a bit) can loop a section of its
  sample, as a sampler's loop mode does: it plays into the section, then repeats it with a
  crossfade at the seam (a 32nd, about 43 ms at 174 bpm) or plays it back and forth (about a
  third of the loops). Part of the seed like ratchets and triplets, beside them in the same bar:
  the seed's loop tendency times Variation sets how many long notes loop (none at Variation 0);
  the section starts a 32nd to a quarter into the note and lasts, as played at Normal Rate, a
  quarter triplet, a dotted 8th or a quarter (rendered half that; shorter ones sounded like a machine gun, 2026-10-20), at least two rounds of it fitting the note. Drawn after density and length
  shaped the notes, before Splice thins the bar; Rate scales the section with the note.
  Reversed segments (2026-10-20): about 40 % of the block seeds reverse those long notes instead
  of looping them, one or the other per seed, as often as the loops would be (Variation): the
  note plays its own stretch of the sample from where it would end back to where it starts.
- **D44** **Squash** (2026-10-20): the lane's last stage, after Heat and Mono lows, a six-band
  compressor pushing each band down above one threshold and up below another (OTT style). The
  knob (left column, OUTPUT) runs from mild (gentle ratios, high thresholds, fast times) to
  strong: on the mudpie hard up and down compression with a smiley balance, as Mesh live's bass
  multiband, the mids kept up (2026-10-20); on the drums much lighter, the attacks kept at a
  128th for the punch. The output keeps the input's loudness (Heat's level match, but without
  K-weighting's extra weight on the highs, so the lifted top does not turn the body down,
  2026-10-20). Not seeded.
- **D45** **Musical times follow the tempo** (2026-10-20): envelopes, decays, glides, the choke,
  the LFO, pitch envelopes, mod-out envelopes, release tails, Squash's attack and release and the
  loop crossfade are note values written for 174 bpm (`tempo::note`) and stretch with the host
  tempo. Groove also gives the lane's own times (the sampler's, not the effects') a feel:
  triplet below 35 %, straight to 65 %, dotted above. Anti-click fades, smoothing and analysis
  windows stay in milliseconds.
- **D46** **Splice as a relation** (2026-10-21): besides its hits, a pitched host (a bass
  instance) shares its line, the key and length of the note on each 16th, and every mesh host
  its form position (form, pattern seed, pattern bar). The Splice knob is then one relation
  axis; the rhythm stays as D42 says (neutral in the middle).
  - **Blocks (B2).** The follower plays the host's form position, so its blocks (letters,
    swaps, fills) change with the host's; its own letter seeds give the content. (Superseded
    by D48: the follower derives a form of its own from the host's by the relation.)
  - **Pitch (B7).** Each note of the bass walk is weighed by how it moves against the host's
    line, the host's last note carried through its rests. Stacked (0): unisons, octaves and
    fifths and motion the host's way are favoured. The middle: free lines, but no parallel
    octaves, fifths or unisons. Gaps (1): contrary motion, holding while the host moves fast,
    similar motion rare and parallel perfect intervals almost never (music-theory §2). The
    weights blend smoothly along the knob and stay seeded, so bounces repeat; the register and
    root rules stay (the bar's first note is the root, the sub does not jump octaves).
  - **Length (B1).** The share of stabs and, after the speed, each note's length move towards
    the host's (stacked), some contrast (middle) or the opposite (gaps: a stab against a long
    host note, a held note against a short one). The bar's first note stays an anchor
    (music-theory §3.3); figures and slid notes keep theirs.
  - A drum host gives the blocks and the rhythm; the sidechain gives the rhythm only. The link
    table moved to `mesh-vst-link-v2`: instances of this version and older ones do not see
    each other.
- **D47** **Form operators** (2026-10-21). The form stays the deterministic big picture: 64
  bars a pass, a varied second pass (128 bars). A and B are distinct motifs that can ask and
  answer each other (`A1q B1a`); C and D are the stark changes, mostly at the transitions (the
  16-bar seams, the end of 64). Related material comes only from **operators**: marks after a
  letter that play its motif changed, often and in short steps, between focused repetitions.
  (D57 replaced the timing marks: the set below is as it was; see D57 for the current one.)
  - **Grammar.** A segment is a letter, its marks and a length in any order, the length one run
    of digits: `A2r`, `Ar2`, `A'7`, `A2'r`, `A3rd`, `Bq.5`. Marks chain left to right (`rd`:
    reversed, then dotted); a later mark wins where two set the same thing. `'` (another seed's
    second half) and `*` (another sound) are marks like the others. Up to six per segment.
  - **Reference.** A segment with operators plays its letter's **first bars** (as the letter's
    segment first plays them: its chords, fills, its first bar's phrase), as many as the segment
    is long, transformed over the **whole segment** (a 3-bar `r` reverses three bars as one), and
    each bar plays its part. So a variant can be any length: `A A Ar Am A2r B`. Spans hold up to
    16 bars (longer ones repeat); the transformed span is cached while its bars render. The
    knobs, the letter's parameter moves, its block speed and sound apply as always; the Splice
    filter applies after.
  - **Operators** (pitch ones on the mid line in the key's scale; the downbeat roots stay):

    | Mark | Variant | At Variation 0 | At 100 % |
    |---|---|---|---|
    | `r` | the pitch order reversed, the rhythm kept | the last beat | the whole span |
    | `i` | inversion: the steps mirrored in the scale around the first note | the steps at half size | full mirror |
    | `t` | sequence: moved by seeded scale steps (2, 3, −2, 4, −3, 1) | the second half | the whole span |
    | `c` | counter line: contrary moves (at most 3 steps) | every second move | every move |
    | `u` / `w` | the line an octave up or down | the last beat | the whole span |
    | `q` | question: the ending opens (the 2nd or the 5th, half the length) | the last note | the last 3 notes stepping to it |
    | `a` | answer: the ending closes (the root, held to the end) | the last note | the last 3 notes stepping to it |
    | `R` | the rhythm mirrored, the pitch order kept | the last beat | the whole span |
    | `o` | rotation by seeded 16ths | by one 16th | up to a bar |
    | `d` / `e` | onsets and lengths on the dotted-8th or 8th-triplet grid | 30 % of the notes | all |
    | `y` | the beats (not the downbeats) a 16th early (anticipation) | 30 % | all |
    | `h` / `x` | half time (the first half stretched) or double time (compressed, twice) | the last bar or beat | the whole span |
    | `l` / `s` | held into the next note, or short (×0.7 .. ×0.3) | a little | in full |
    | `n` / `p` | weak notes dropped (15 % .. 60 %), or passing notes in wider steps (20 % .. all) | | |
    | `m` | mutation: seeded notes change (a neighbour, an octave, half or twice as long, gone) | one note | a third of them |
    | `k` | split (D51): a hit repeats fast over itself (the double feel), straight 16ths at low and middle Groove, a triplet or a dotted 16th at high Groove; on the drums a rising 32nd roll | one hit over an 8th | two hits over a beat |
    | `'` | another seed from this beat on | the last beat | the second half |
    | `?` | the seed picks one by the segment's place: double time (or `h d e n`) before a change, an answer (or `i q m r`) at a period's end, a mutation (or `r o y l`) on a repeat, else any (in a written form; a generated form picks by D51) | | |

    Variation sets how far every operator goes, from a little (0 %, never nothing) to the full
    variant; the form's second pass goes 15 % further (it escalates). Standalone, no rule
    keeps notes off the snare steps.
  - **Drums.** `m` turns hits into ghosts, drops off-beat hits or adds a ghost after one; `o`
    displaces by an 8th; `q` opens the ending (the last hits gone), `a` closes it (a strong hit
    on the last beat); `r` and `R` mirror the kit's rhythm; `d e y h x n s` work on the hits; the
    pitch operators do nothing. **TODO: better drum operators** (the user wants better ones).
  - **Splice.** A follower plays its host's form position (D46), so its host's operators too.
    Towards the counter end its questions and answers swap roles with the host's (the chance
    rises with the relation). Against a pitched host each bar as it plays (after the operators,
    the speed and the thinning) is fitted to the host's line again: each note (not the downbeat
    root) may move up to two scale steps, or to the host's move followed or mirrored, when that
    fits the relation clearly better (the walk's weights).
  - **Forms.** Ten 64-bar forms, each its own character, one line per 16 bars ending on a
    transition: steady roller, question and answer (2-bar periods, answers in the last half
    bar), slow build, two-step cells, breakdown (fillers in half time), B-led, stabs and cells,
    space (thinned and held), switch-up (the second drop re-cuts the motifs), bounce.
    (Superseded by D48: the forms are generated; these ten became the characters of its fill
    plans. The grammar and the operators stay.)
- **D48** **Generated forms** (2026-10-21). No more built-in forms or Form parameter: a form is
  built from three seeds, each rolled on its own, so a wrong part can be targeted (the
  phrases, how the 64 bars are built from them, the second pass). `arrange.rs`.
  - **Phrases.** A, B, C and D are phrase slots, the letter is the type: A asks, B answers, C
    is a short fill, D a bigger transition. A phrase is its letter's notes (from the pattern
    seed, as before) looped over a seeded length in **beats, a power of two**: A and B 2, 4, 8
    or 16 beats (weights 15/35/35/15 %, at most 4 bars), C and D 1 to 8. It loops from its own
    start (written `A2:8`: two bars of an 8-beat phrase) and starts a seeded **offset** into
    its letter's notes (`+2`), one per letter, mod the length, shared by the letter's variants.
    Offsets are even beats (the kick's beats stay the kick's); a pitched phrase starts at the
    even beat at or after its offset where a note starts (a note a 16th earlier is clipped onto
    the beat), else at the letter's start, so it never starts late. Generated drum phrases have
    no offset. A segment entering its phrase later (a fill
    took its head) skips in (`^2`). A pitched phrase is written on the tonic and moves with the
    song's chord where it plays, by scale steps the nearer way (the riff sequenced over the
    progression); against a pitched host it is walked on the bar's chord instead. The **phrase
    seed** draws each letter's phrase; a **pin** per phrase keeps its seed through phrase rolls.
  - **Shape** tilts the lengths (low Shape short phrases and cells, high Shape long ones: a
    bias on the seeded pick, the mean length rises with it), **Variation** scales the form's
    variation: the share of the plan's `?` slots a melodic operator fills (25 % .. 85 %; the
    rhythm operators are Density's and the Groove's, D51), extra fills inside a
    section (4 % .. 30 % per 4-bar seam) and how far the second pass reaches. Both are read in
    five bands, so a small knob move keeps the form.
  - **Fill plans** (the **fill seed**): ten characters from the old forms (steady roller,
    question and answer, slow build, two-step cells, breakdown, B-led, stabs and cells, space,
    switch-up, bounce). A plan gives each 16-bar section one or more **units** (`A A A? B?a`,
    `Aq Ba`) and its seam fill. A C or D seam may draw its kind from its place's pool instead
    (`C* Cx Ce Cd ...` at bars 16 to 48, `D* Dx D*x ...` at 64), at 35 % .. 80 % over Variation
    and more often in the second pass; A and B seams and empty ones keep the plan's character.
    The seams at bars 16 and 48 are minor: a fill plays there at 20 % .. 50 % over Variation and
    lasts at most 2 beats; at 32 up to 4 beats, at 64 up to 8, both always.
    A unit repeats over **16-beat cycles** (a unit longer than 16
    beats takes a cycle of 32 or 64), each cycle starting it again, the last phrase clipped, so
    unequal A and B lengths still fill a cycle; a seam fill pads what a short unit leaves of a
    cycle, else it overwrites the section's last beats; a pickup may open the form; rare breaks
    sit inside units (`A Ao C B?`) or come as extra fills at 4-bar seams. Every section is built
    on its own and fills overwrite, so the **16-bar grid holds by construction**. Per form the
    seed either varies the unit shapes per section (the letters keep their lengths) or the
    letters' lengths per section (the shapes stay), never both. Operators live in the plan
    only: fixed marks and `?` slots, resolved by the fill seed (D51: rhythm operators by the
    letter's Density and Groove lean, melodic ones by their place).
  - **Second pass** (the **second-pass seed**): bars 65..128 are the plan built again by one
    seeded recipe, a second one higher up Variation: switch-up (A and B re-cut by `r i d o R`
    from a section on), escalate (more slots and fills, pushes `x y p`), order swap (A and B
    trade places), new answers (B plays `t i c p` on top), hook back (a section cut short, the
    opening's unit back halfway), double end (the last 4 bars doubled up), thin drop (the pass
    opens thinned). Variation sets how many sections a recipe reaches. It replaces D47's
    letter swaps. A written form may give its second pass after `//`.
  - **Rules** (music theory §5, §6.3), checked in code: the grid; inside a section each cycle
    changes at most once against the one before (enforced while the slots resolve, against
    the same letter under its beats when a fill or the pickup moved the starts); the
    structural repetition at the hook period (4 or 8 bars) between 0.05 and 0.95 (0.99 at
    Variation 0); a busier second pass. (D51 dropped the rule that the first 8 bars repeat
    unchanged: A is the question and may carry operators from bar 1.) A form that breaks one
    builds again from the same seeds with its slots, fills and recipe drawn anew (up to 12
    attempts, the fewest broken rules win; 0 of 10 000 forms break one at the defaults, at
    most 1 at the knobs' extremes). Measured on the rendered bass, consecutive 4-bar blocks agree
    in pitch at a median of about 0.6 (the references: 0.33 .. 0.63).
  - **Parameters.** Phrase, fill and second-pass seed are automatable parameters (all others
    are too now, user 2026-10-21); the rolls only write seed values. A seed or band change
    builds the form on the audio thread without allocating (about 10 µs, at most 13 attempts)
    and plays from the next bar rendered (the old Form parameter switched the same way; the
    4-bar boundary of `FORM_CHANGE_BARS` is not enforced). The pins are project state.
  - **Splice.** The link (v3, `mesh-vst-link-v3`; v2 instances do not see it) carries the
    host's form seeds and bands; the follower builds the host's form and derives its own from
    it per 4-bar window by the relation: stacked, it takes the host's window (85 % at the end),
    else its own; in the middle its own; in the gaps (up to 80 % each) its fill leaves a window
    where the host fills, a fill goes where the host plays on, its variants go where the host
    holds and stop where the host changes. The relation is read in nine bands. A follower
    without a form of its own plays the host's. D46's note relation and D47's question and
    answer swap stay.
  - **Contrast** (D49) is read in five bands too: it sets how often B's phrase is a step off
    A's, and the link shares its band.
- **D49** **Contrast** (2026-10-21): one knob for the question and answer, in Permute's place
  on the panel (Permute stays an automatable parameter, at 0, and the mod wheel still drives
  it). `contrast.rs`.
  - **A plays the knobs.** B's notes always come from its own seed. At Contrast 0 B plays A's
    settings exactly; towards 1 it moves away on every letter axis (density, complexity,
    variation, slide, Spread's register and jumps, accents, Groove's swing and syncopation,
    chop, mutation, Shape's length, Morph, Permute), each in a direction its seed picks, so the
    seed decides which letter is the long, the busy or the high one. Near a knob's end B moves
    inward and less (half its move at the very end, all of it from a quarter in), so the
    knob's character stays: at Shape 0 both letters stay short, at full both stay long.
    Morph is the exception: B's start in the sample moves by Contrast alone, whatever the
    Morph knob, wrapping around the sample (up to half of it).
  - **Steps.** B's bass speed is one step off A's with a chance up to 60 %; its phrase one
    power-of-two step off A's length with a chance up to 70 % (the D48 grid stays); its line
    may answer mirrored (`i`, up to 50 %, not against a pitched host, where Splice sets the
    direction) or an octave up (`u`, up to 35 %). All are seeded per letter, so all of B
    answers alike.
  - **Fills.** C is the mild fill and derives from A: a seeded move of 0.25 .. 0.6 of the
    largest by Contrast, half the filler boosts. D is the aggressive one and derives from B:
    0.5 .. 1 of the largest on top of B's settings, 1.3 times the boosts, and more often a
    speed step up. At Contrast 0 both still vary, less than before; at 1 about as before.
  - **Variation** no longer moves letters apart (the 0.6 .. 1.4 letter reach is gone). It sets
    what changes inside a pattern and a form: per-note moves, loops and reversals, operator
    slots and amounts, extra fills, the second pass's reach. The second pass's small move is
    now the same for every letter (from the pattern seed), so B still plays A's settings at
    Contrast 0 there.
  - **Splice** stays the relation between instances. Layered on a pitched host, the fit after
    the operators may also take the same note an octave off to follow the host's way
    (contrary motion on the layer side fell from about 0.46 to 0.14 in the relation test).
- **D50** **Bass clips** (2026-10-21): the bass plays from MIDI clips instead of generating its
  line, so its riffs are musical first and the knobs shape them. `clips/`, the bass's
  `clip_bar`.
  - **Source.** An automatable **Bass source** setting, **Clips** (the default) or
    **Generated** (the line as before), switched next to Rate for A/B listening. The drums
    stay generated.
  - **Clips.** A clip is taken as written, any length: its length is the MIDI file's end of
    track (or its last note's end, if later), rounded up to the next beat, leading and trailing
    rests included. It plays one voice (the lowest note of a chord). Its root is detected
    (the motif key detector) and its notes kept as semitones over it. When it plays, the root
    lands on the key's root and every note outside the key's scale (and mode) moves to the
    nearest scale note (between two, the one at its own scale step); a key change follows at
    once. The phrase follows the song's chords by scale steps, as generated phrases do.
  - **Shipped.** Ten clips per letter, 32 beats each, written on C minor from the
    music-theory note (root-heavy, the downbeat mostly on the root, the dark intervals
    sparingly, bars 4 and 8 their endings, each with its own rhythm). Long and short notes mix
    in every clip, and the notes sit off the beat too: on the e and the a, offbeat 8ths,
    anticipations held across the beat, 3-3-2 and dotted groupings (34 % of the onsets on a
    beat, 59 % in the first set; user, 2026-10-21: the first set was staccato and all on the
    beat). A states and asks, B answers home to the root or the 5th, C are milder fills and
    turnarounds, D the aggressive transitions (accents, octaves, the b2 and b5, chromatic
    pushes). Per bar a table gives the 16-step rhythm (`X` held, `x` a stab of half its span,
    `A` `a` accented, `g` a ghost, `-` continues, `.` rests) and the onsets' pitches; the
    velocities follow the metric place (downbeat 112, beats 104, 8ths 96, 16ths 88, held +4,
    the phrase points of bars 4 and 8 +6, accents +14, ghosts 72; 72 .. 124) and are written
    into the MIDI. Ties across bar lines are not written: a note ends at its bar line. They
    are tables (`clips/tables.rs`), written to `crates/core/assets/clips/<letter>/NN.mid`
    by `cargo run -p mesh-vst-core --example write_clips` and embedded; a test keeps the files
    and the tables equal.
  - **Your clips.** `*.mid` files in `~/.config/mesh-vst/clips/a` .. `d` (one folder per
    letter) join the shipped ones when the plugin loads (up to 64 per letter; unreadable files
    are skipped). Any key and length work; one voice is played, the lowest.
  - **Windows.** The letter's seed picks its clip. A phrase (D48: 2 to 16 beats) is a window
    anywhere in the clip: a seeded 16-beat part of it plus the phrase's offset, from the even
    beat at or after that where a note starts (as D48's pitched phrases); a clip shorter than
    the phrase repeats. Variants, operators and pins work on the window as on a generated
    phrase.
  - **Velocity** comes from the MIDI, with no random accents (user, 2026-10-21: far too
    random, the loudness between notes differing far too much). The written dynamics are
    fitted into a **velocity window** around the clip's mean, 8 at Variation 0 to 20 at
    Variation 1 (the most two notes of a bar differ); the accent parameter flattens them below
    its middle and fills the window from there, never beyond it; notes written from 116 up
    are the accents. Every note of the bar, fills included, stays inside the window. The
    generated line keeps its accents mostly on the strong steps (their chance scaled by the
    metric weight, at most 0.5 of the accent) and is fitted into the same window; its seeded
    velocity jitter is ±2 (was ±5, the drums' too); the bass's accent gain is +0.5 dB (was
    +3), so the window is what the ear hears.
  - **The knobs** shape the clip in the directions they shape the generated line. Density at
    its middle plays it as written; lower drops the weak notes (the bar's first note and the
    beats stay), higher splits long notes into 8ths, 16ths near the top, of the same pitch,
    and **fills the rests** after a note: on the 8ths from 0.55 (90 % at Density 1), on the
    16ths between them from 0.8 (80 % at 1), so full Density sounds most 16ths. Fills follow
    the Groove's feel (triplet 16ths at low Groove, straight in the middle, dotted at high;
    the swing reaches them as the rest of the line), repeat the note before them a quarter
    (8ths) or 0.4 (16ths) of the window softer, held to the next grid point (near legato);
    Variation skips up to 35 % more places and turns up to 40 % of the fills to the octave or
    a scale neighbour (low Variation: plain repeats).
    Spread lifts weak notes an octave; Syncopation (and Complexity above its default) pushes
    notes off the 8ths; the bass's stab amount shortens held notes above its middle and holds
    stabs below it; Register moves the octave; accents, slides, Shape's length and legato,
    the filter, Variation's loops and reversals, Contrast, the operators, the speed and Splice
    act as on the generated line. A drum host's kicks thin the clip (avoid) or add the root
    (double).
  - **Loudness.** The bass is calibrated through Heat and Squash as well (measured raw, then
    once more through them at that trim), because sustained clips leave the Squash up to 3 dB
    quieter than it should be; the Squash knob recalibrates in 5 % steps.
  - **A mono note whose gate ends at a new note** no longer glides into it (it played on from
    where the held note got to, past the sample's end: a silent bar). The new note strikes.

- **D51** **Rhythm operators by Density and Groove** (2026-10-21; superseded by D57: the form
  places no rhythm operators, the chain's Groove and recursion do the rhythm). The seeded `?` slots of a
  generated form (D48) split in two kinds. `arrange.rs`, `ops.rs`.
  - **Rhythm operators** (`x h k y d e o R`): **Density** sets how many: the chance a slot
    plays one (5 % at Density 0 .. 85 % at 1), and a plain repeat of A or B may take one too
    (0 .. 30 %); the plan's own rhythm marks stay at 30 % .. 100 % over Density, so low
    Density calms the plan's character. The **Groove** biases which: low Groove the straight
    ones (double time, half time, `k` in 16ths), high Groove the funky ones (syncopation,
    dotted, triplets, rotation, `k` in triplets or dotted 16ths), a blend in the middle;
    double and half time 1.5× as likely before a section change; `R` at either. The draws go
    by the place in the section, whatever the letter. Density and Groove are read in five
    bands like Variation, Shape and Contrast; the link shares them (a padding byte).
  - **Melodic operators** take a slot no rhythm operator took, at Variation's share (25 % ..
    85 %), by the place (D47 without the rhythm ones: before a change `n q t u`, at a period's
    end an answer or `i q m r`, on a repeat a mutation or `r l p`, else `r i t l s m p`).
    Variation sets how far every operator goes, as before, but no longer how many rhythm ones.
  - **Rhythm first:** in every generated chain, every letter, the rhythm operators come before
    the melodic ones (the rhythm moves, then the line changes on it).
  - **Contrast** parts the letters' rhythm (continuous): at 0 every letter takes A's lean and
    count (the knobs'); towards 1 B moves to the other side of the Groove (at least 0.4 off
    the middle in full) and its count away from A's (up to 0.4), C and D each move in their
    own seeded direction (up to 0.3 on both).
  - **`k`** (split) is a new operator (see D47's table).
  - **No opening rule** (user: A is already the question): the first 8 bars may carry slots,
    extra fills and their changes.
  - The second pass adds 15 % to the rhythm chance (and its recipes' own marks).
- **D52** **Full Density fills, Variation as set, Shape's half time, Splice answers**
  (2026-10-21). `pattern.rs`, `bass.rs`, `arrange.rs`, `ops.rs`, `loader.rs`, `engine.rs`.
  - **Density** near the top fills the bass line's rests after its operators too (8ths from
    0.55, 16ths from 0.8, at 1 nearly all): a fill repeats the note before it (the bar's last
    before its first) at its length, softer. The speed limit (Normal and Half at 8ths) keeps a
    note pushed up to a quarter of the gap early; it merged every other pushed 8th before.
    Split repeats and fills end where the next note starts (they ended 2 ticks early, every
    note restruck after a gap). Variation skips fewer fills from Density 0.9, none at 1.
    A Splice follower towards the counter end keeps its rests; where Splice drops a note at
    high Density the note before holds over it (not a stab, up to the next note).
  - **Variation** plays as set: no letter, fill, second-pass or Splice move. Variation now
    moves **Morph** per letter: B, C and D each a seeded way off A's, up to 0.6 knob values
    at Variation 1 (curve 1.5).
  - **Bass attack** follows Shape, not Density: 5 ms at 0, log-spaced, about 22 ms at the
    middle, 100 ms at 1. The stab pitch blip plays on `STAB` notes only (every 8th had it).
  - **Shape's operator:** above Shape's middle half time joins the rhythm pool (weight 2.5 at
    full Shape). B and D lean to the other side of the Shape by Contrast (in full at 1): a
    stabby A answers with a held, half-time B.
  - **Splice answers** (counter side): a 4-bar window plays the other main letter where the
    host plays one (B against A, A against B), each with its own phrase, at a chance of 0.7 ×
    the relation. Two instances on the same seeds now differ in pattern, not only in thinning.
  - **Streams** (mudpies) end where they stay 12 dB under their overall level to the end, so
    the level map (±18 dB) evens out every start position. A one-shot chord as a mudpie
    played its decayed tail 20 .. 40 dB down.
  - **Short at Shape 0** (follow-up, user: almost only held notes): a note up to a stab long
    shortens too (to 35 % at Length 0), the release follows Shape below its middle (1/64 at 0,
    the designed one at the middle; it was ≥ 3/32 and rang over every 16th), legato `l` holds
    less below the middle and half time keeps the notes' length at Shape 0. A mono note
    ending within half a tick of the next one strikes it (no glide). The Density refill runs
    after the speed, on the grid it allows (8ths at Normal and Half, 16ths at Double), so a
    letter one speed step slower fills too. A bar's later part (a letter change mid-bar)
    brings its note held over the boundary in on the boundary; the earlier part's note ends
    there (it was lost, a silence up to half a bar).
  - **Snapshotted operators (see below).**
- **D53** **Neuro** (2026-10-22, fourth draft). One automatable slider on the bass's right
  column, 0 % no processing (bit-exact) .. 100 % a heavy resampling-style chain. `neuro.rs`,
  first on the bass lane's output; the bass then runs mono lows, Squash and **Heat last**
  (the drums keep Heat before Squash; the bass calibration follows the new order; D59 puts
  the bass's Squash after Heat again). The
  unison detune is in the sampler, before all of it.
  - **Split:** below 130 Hz the sub passes clean; the band above runs the chain and is
    level-matched back to its own loudness (K-weighted, 0.1 .. 3×), so the slider changes the
    character, not the level.
  - **Chain** (`ORDER`, one `Stage` each, reorderable in one place): emphasis (broad boosts,
    +8 dB Q 0.5 at half and +7 dB Q 0.6 at 2.4× the cut, around a narrower, deeper cut,
    −18 dB Q 1.6, resting on the note's 6th .. 16th harmonic within 300 .. 1500 Hz) → stereo
    chorus (before the drive) → symmetric drive (tanh into a sine fold, up to 24 dB) →
    deemphasis (the boosts cut back by 80 %, the cut restored by 35 % and broader, its bands
    moving their own ways) → two notches moving against each other (broad −18 dB Q 1.2 in
    250 .. 500 Hz, narrower −20 dB Q 4 in 1 .. 5 kHz) → a subtle asymmetric drive (diode-like,
    even harmonics, offset blocked) → damped comb (feedback 0.55, a 2.5 kHz lowpass in its
    loop, mixed at half, on the note's 2nd .. 6th harmonic) → air (peaks +6 / +5 dB and a
    null −12 dB in 2 .. 8 kHz, none at a note's onset) → phaser → a slight Chebyshev waveshaper (3rd and 5th) →
    post lowpass → a slight multiband compressor (the Squash at its mild end, mixed in up to
    60 %).
  - **Motion** comes from the sound only, no LFO (user, 2026-10-22). Each note gets an ADSR
    whose shape the seed picks (snap, swell with a long release, slow, pluck, ±30 %) and
    whose times are shares of the note's length, known at its onset: the release ends with
    the note, stabs move fast, held notes slowly. An accent opens it fully; a slide carries
    the note on to the slid-to note's end. Every filter band (emphasis, deemphasis, notches,
    air, the phaser's two sides) has its own seeded mover: a resting offset, how far and
    which way the ADSR moves it, the note's draw (Variation), Morph. How a stage's bands move
    against each other is a relation the seed picks per stage: together (all one way), apart
    (the lower down, the upper up) or over each other (a peak and a null travel 65 % of their
    gap each way, so they pass at the ADSR's ends; the notches, in fixed ranges, move apart
    instead). Boosts stay at least an octave apart (an upper one too near moves up). The chorus delays move with the ADSR too (the detune bends with the
    note). The air grows over 60 % of a note. An envelope follower on the input (6 ms against
    0.3 s) sets how hard every stage works: louder parts of a note more, quieter less (a
    seeded 50 .. 100 % of that), and the movers' gains with it.
  - **Neuro** raises the depth of everything (peaks, nulls, wet, drive, detune) and the speed
    only a little (1 .. 1.5×); Variation scales the depth (0.4 .. 1.2×) and the speed
    (0.8 .. 1.3×).
  - **Contrast** parts the letters (D49's shape): at 0 all play A's movement; towards 1 B
    turns its bands' directions, places them up to an octave off, takes the most different
    ADSR shape (two along), another emphasis harmonic, the comb's other sign, other band
    relations (each stage's changes at a chance of Contrast, every one at 1), and moves its
    attack and decay at another speed (0.4× or 2.5×, seeded); C moves 40 % of the way off
    A (relations too); D takes B's and goes half as far again in its own direction and speed.
  - **Source and register** (user, 2026-10-22: a lead in mudpie mode sits above the bass
    ranges; 1 kHz and up turns harsh when driven). The library's scan keeps per file its
    centroid above 130 Hz, the sub's share of all (below 130 Hz) and the share above 1 kHz
    (of the band above the sub); older caches scan again. Playing, the followers hear the
    same shares at the note's actual pitch and take over from the scan once they have heard
    enough (a file played far from its root has another sub). The sub's share sets how
    bass-like (−3 dB or more) or lead-like (−15 dB or less) the source is; the share above
    1 kHz how harsh (8 % .. 45 %). The register (the scanned centroid moved to the note,
    from 700 Hz; else the note from 65 Hz) moves the moving ranges up by 0.8 of each octave,
    a bass-like source up to 3 octaves, a lead-like one up to 1 (its air does not move: a
    first draft moved a bright lead's ranges up and made it harsher, 6 kHz+ from −12.7 to
    −5.1 dB). A harsh source gets up to half less drive gain; a lead-like one half the heavy
    drive and half again of the subtle ones.
  - **Clean and balanced:** before the drive a high shelf at 1.2 kHz (moved with the
    register) cuts 6 .. 12 dB by harshness, half of it given back after the deemphasis, so
    the drive bites on the mids. The three drives run at 2× oversampling (a fourth-order
    Butterworth up and down, dry and wet mixed at the high rate). The processed band is
    highpassed at 130 Hz again (the drives' difference tones stay out of the sub). A balance
    guard (fourth-order measurement, 50 ms): the share above 1 kHz may grow 6× over the
    input's (2× for a lead-like source, never held under 8 %), the share above 3 kHz 10×
    (never under 6 %) and never over 25 % (15 % lead-like); high shelves take back the rest.
    At full: a chord as a mudpie 6 kHz+ −34 → −12 dB, a lead −12.7 → −10.5, the bass mudpie
    −66 → −42. Coefficients update every 8 frames.
  - **Per seed:** the sound seed blends two of four racks (talker, metal, tear, reese: how
    much of each stage the slider brings in) and sets the movers, the ADSR shape, the
    emphasis and comb harmonics, the comb's sign, the fold share and the follower's share.
  - The slider's curve comes in early (`amount^0.8`, a rack weight of 1 in full at about
    80 %). MUSICAL: every depth is a draft. User, 2026-10-22: the first draft's peaky bells,
    its comb, the formants and the ring modulator sounded bad (formants and ring are gone).
  - Open: tonalize (partials snapped to the key) and cross-synthesis with a sidechain or a
    Splice host as further spectral stages; drums get their own chain later.
- **D54** **Multiband distortion** (2026-10-22). `mbdist.rs` (shared blocks in `dsp.rs`): a
  processor like the other stages, configured per use (`MbConfig`: two Linkwitz-Riley
  crossovers, per band a curve, drive, wet, containment and oversampling). User: subtle
  multi-stage distortion beats one stage, so it replaces the heavy stages where a band split
  helps and the subtle ones stay; never all wet (at most 85 %), stronger drives mixed in under
  the dry instead.
  - **Per band:** the band's level is followed and its input normalised to a working level
    before the curve (the makeup, from the RMS of a driven sine, undoes it), so a quiet band
    distorts like a loud one; a band with under 8 % of the signal gets less wet (nothing there
    to grow but noise). Curves: tanh, an asymmetric tube (its offset blocked), a sine fold, a
    hard clip. A containment lowpass on the wet at a multiple of the band's top keeps its
    harmonics out of the top octaves. 1×, 2× or 4× oversampling (4× = two 2× stages). The low
    band passes the upper split's allpass, so the bands sum flat when nothing distorts.
    Distorting the bands apart also keeps the mids and the highs from intermodulating.
  - **Neuro (D53)**: the heavy drive is multiband: crossovers 600 Hz and 2.5 kHz (moved with
    the register), a tube on the low mids (20 dB, 65 % wet, contained at 6× its top), the
    growl on the mids (a fold or a hard clip by the voice, 28 dB, 75 %, 4×), a soft tanh on the
    highs (8 dB, 35 %, less on a harsh source). The subtle asymmetric drive stays (16 dB, at
    most 70 % wet), the shaper too. At full: the chord as a mudpie 6 kHz+ −34 → −15 dB, the
    lead −12.7 → −10.2, its 3 .. 6 kHz down (−11.9 → −20.5).
  - **Heat**, both lanes: clip, then the tilt pre-emphasis, the multiband, the tilt undone,
    the crush last (its steps stay sharp), the level match. Bass: 150 Hz / 1.5 kHz, a gentle
    tanh on the low band (8 dB, 30 %: the sub stays solid), a tube on the mids (30 dB, 80 %),
    a soft top (18 dB, 50 %). Drums: 200 Hz / 3 kHz, a tube on the kick's band, a hard clip
    on the snare's (24 dB, contained), a soft top, all at 25 .. 35 % wet under the clean hits
    (the transients stay). Tone moves drive between the low and the high band (±9 dB) on top
    of the tilt.
  - Open: a config per drum slot (kick, snare, hats on their own buses) instead of the
    lane's mix.
- **D55** **Neuro and Heat revised** (2026-10-23). User: the filter movement was not heard at
  full Neuro, the bands' curves and crossovers were wrong, the pre-emphasis was not heard.
  - **Bands in passes:** a band runs one or two curves in series (`Pass`: curve, drive, wet),
    new curves tape (a biased cubic knee) and drive (pedal diodes, an exponential knee, its
    kink grows upper partials). The containment filters only what the curves added.
  - **Neuro drive:** 150 Hz / 1 kHz (moved with the register): tape 4 dB at 35 % below, the
    drive 24 dB (±4 by the voice's fold share) at 70 % then a slight tanh (8 dB, 40 %) on the
    mids, a slight tube (5 dB, 20 %) on the highs. Below 130 Hz Neuro passes clean, so the tape
    band only holds 130 .. 150 Hz there.
  - **Heat:** bass 150 Hz / 1 kHz: tape, a tube (24 dB) then a slight tanh, a slight tanh on
    top. Drums 150 Hz / 1.2 kHz: tape, the drive (18 dB) then a slight hard clip, a slight
    tube on top, all at 20 .. 35 % wet.
  - **Notches are gaps:** two crossovers take the band between them out (`dsp::Gap`, summing
    to an allpass when kept). The low one 1.5 octaves wide at 24 dB/octave (Linkwitz-Riley)
    with a −12 dB bell in its middle (about −25 dB there), centred in 250 .. 500 Hz; the upper
    one 1.25 octaves at 18 dB/octave (third-order Butterworth, −23 dB on its own) in 1 .. 5 kHz.
  - **Emphasis:** bass-like ±12/−24 dB, lead-like +7/−16/+5 (blended by the source); the
    deemphasis gives back under half (45 % of the boosts, 20 % of the cut), so it stays heard.
  - **Movement:** the ADSR's sustain slopes over the held part (seeded sign), the ADSR reaches
    0.7 .. 1.6 octaves, the note's draw 0.7. Measured at the output (after every later stage,
    the compressor too), 10 ms frames over four seeds: the low gap's centre spreads 0.3 .. 0.5
    octaves, the upper one 0.7 .. 0.9 (before: 0.13, sitting at 1.35 kHz).
  - **Second pass (user, same day):** the movement was still too short, one move per hit, and
    the sound nasal with too much around 2 .. 5 kHz, the low cut too hard.
    - **Sweeps:** a sweep lasts at least one or two beats (seeded, B the other at high
      Contrast); notes starting inside one do not restart it, so stabs move through it
      together. The ADSR shapes' attacks are 8 .. 60 % of the sweep, so an attack sweeps
      audibly too.
    - **Emphasis in the middle:** the cut sits in 250 .. 600 Hz, every band at most 1 kHz for a
      bass-like source (2.5 kHz lead-like, up to an octave more with the register); the upper
      boost +6 dB, three quarters of it given back after the drive.
    - **Less nasal:** the phaser's feedback 0.6 → 0.3, the drive's harmonics contained at 3 kHz
      on the mids, the subtle asymmetric drive 16 → 9 dB at 45 %, the air's boosts +3 / +1 dB.
    - **The low gap** takes out at most 75 % with a −4 dB bell (about −12 dB in its middle),
      its shoulders just outside the edges boosted +4 dB, moving with it.
    - **Follower lowpass, first in the chain** (user): a 24 dB/octave lowpass (Q 0.54 and 1.9:
      a little resonance) on bass sources only (none on a lead-like one), opening fully (20 kHz); its
      closed point is the seed's, 800 Hz .. 8 kHz (B its own at high Contrast), rising
      towards the top at less amount. On the envelope follower alone it stood still on
      a held mudpie (user: static, muffled), so the sweep's ADSR sets three quarters of the
      opening and the follower the rest. At full, the brightness (share above 2 kHz) now
      spans 28 dB between quiet and loud frames on one seed (was 11 dry).
  - **Drums' Heat keeps the top:** its crush holds no samples (bits only) and the oversampler's
    cutoff is 0.46 of the rate (was 0.42): noise hits above 10 kHz lose at most 0.5 dB at full
    Heat (was 3.1).
  - **Squash (D44) stronger:** a version with thresholds around each band's own mean muffled
    the sound and brought out the low mids (user); reverted to the absolute thresholds and
    band gains, made more aggressive mostly by lifting more: mudpie strong down thresholds
    1 dB lower at higher ratios (5 .. 8:1), up thresholds 2 .. 4 dB higher at 3.5 .. 8:1, up to
    20 dB; drums strong 1 dB lower down, 3 dB higher up at 2.2:1, up to 11 dB.
- **D56** **Density fills in clusters** (2026-10-23). User: Density filled random places, a
  lone hit every two bars that made no rhythmic sense. `cluster.rs`: a bar's rests on the
  fill grid group into gaps between its notes; Density picks whole gaps (from 0.55, the
  share rising as the square root to all at 1) and fills a run in each, at least two points
  where the gap has them (40 % of the gap at the lowest, all at full): a lead-in ending on the
  next note (65 %, its velocity growing) or a trail after the note before (fading), as hits
  or one held note (clip fills: up to 50 % at Stabs 0 and Shape 1; the refill after the
  operators: 25 % at Shape 1; none at Shape 0). The 8ths fill from 0.55, the Groove feel's
  16ths above 0.8. The draws are per gap: the clip path's follow the phrase rule, the
  refill's repeat every two bars.
- **D57** **The render chain** (2026-10-23, user; supersedes the bar renderer's ad-hoc order,
  the rhythmic operators and the phrase rule). A lane's bar is made by one fixed chain of
  phases, each in its own module (`chain/`), each changing one thing, reading only what the
  phases before it settled. The chain's result is the MIDI the sampler plays. Every draw is
  snapshotted: keyed by the pattern seed, the letter and the note's place in the skeleton,
  never by the bar number, so a letter plays the same way every time (the hard-coded phrase
  rule, three bars repeated and a fourth partly re-drawn, is gone: variation over a phrase
  comes from the form alone).
  - **R0 Resolve:** every letter's own knob values (Contrast moves B, C and D off A, D49), its
    speed step, its Morph move, its seeds. Contrast is decided here, once, as values every
    later phase reads, so nothing after it undoes it.
  - **R1 Skeleton:** the rhythm only: onsets on the 16th grid, accents, a base length (to the
    next onset), the base note (the root; a clip's written pitch, marked fixed). Density
    reveals and thins the skeleton's notes here (the generated figures' levels; a clip's
    weak notes). The kick mode acts here.
  - **R2 Structure:** A′ (a different second half), Permute (chunks swapped).
  - **R3 Speed:** the Rate setting and the letter's speed step on the skeleton, the speed
    limit (double speed keeps 16ths, half speed 8ths). Normal is the generator's own pace
    (no stretch): the pace the user asked half of (2026-10-20) comes from the generator
    writing fewer, longer notes at the centre of Density and Length.
  - **R4 Recursion** (depth 1; Groove sets how often, about 1 in 10 notes at Groove 0.5 and 1
    in 3 at 1; subdivision three times as often as doubling; subdivision prefers held notes,
    doubling short ones):
    - *Subdivision:* the note's span is replaced by a pattern of its own: the letter's
      skeleton made again with a seed of this note (R1 to R3), a window of it (two or four
      times the span, placed by the seed and the note, moved by Variation) squeezed into the
      span, on a grid of 32nds at most. Its notes are a group (`origin` recursion, a group
      id) that is never subdivided again.
    - *Doubling:* the note takes twice its length and the notes it now overlaps are deleted;
      the pattern after it stays where it was.
  - **R5 Density fill:** the rests left after the recursion fill in clusters (D56).
  - **R6 Length** (Shape, Length, Stabs): every note's length. Stab or held by a chain over
    the notes (a stab after a stab likely, about 75 %, a lone stab rarer), the share by
    Shape, Length and Stabs; a held note's start or end may turn into one or two stabs with
    the rest held; legato and staccato as the Shape says. After the recursion and the fill,
    so the knob sets the lengths of every note directly.
  - **R7 Groove:** onsets pushed, pulled and moved off the grid, lengths nudged a little
    (about ±10 %, never over the next note), on every note including the recursion's. Groove
    sets how often and how far (the middle rare and mild, the top often and far); the seed
    and Variation which notes and which way. The drums keep their swing and push here.
  - **R8 Melody** (Spread, the seed, the harmony): the pitches for the finished rhythm, a walk
    (D-walk rules: steps, consonant leaps, gravity to the root, the register's floor and
    ceiling, odd blocks, Splice's counterpoint against a pitched host). A recursion group
    plays its own short line from its parent note's pitch. A clip keeps its written pitches,
    fitted to the key and the chord; its fills and recursion notes take pitches from the
    walk around the clip notes before them.
  - **R9 Operators** on the full notes of the span (D47's span cache): placed by the form
    (Variation's chance, the seed's kind per place, snapshotted) or written in it. Kept:
    `r` the pitch order reversed, `i` the contour mirrored, `t` sequence, `c` counter line,
    `u`/`w` octave, `q`/`a` question and answer, `p` passing notes, `m` mutate, `h`/`x`
    augmentation and diminution (half and double time over the span). New: `b` the span
    backwards in time (pitches, rhythm and lengths), `v` backwards and mirrored, `z`
    displacement (by an 8th), `j` hemiola (three against four), `g` truncation (cut short,
    a rest), `N` neighbour tones, `P` pedal point (alternating with the root), `Y`
    arpeggiation over the chord, `F` fragmentation (a fragment repeated), `K` call and
    response (the span's second half answers its first). Gone, their marks no longer
    parse: `d` dotted, `e` triplets, `o` rotation, `R` reversed rhythm, `y` syncopation (now
    Groove), `l`/`s` legato and staccato (now Length), `n` thin (now Density), `k` split
    (now the recursion). Contrast's B answers (mirrored or lifted) join the chain here.
  - **R10 Splice:** a follower's fit to its host's line (D46), the lengths alike or against
    it, the thinning by the focus, the parts of the bar.
  - **R11 Expression:** velocities inside the Variation's window, accents, slides (ties and
    glide, after the melody so they know the pitches), the note filter, attack, the start
    region and Morph and pan per note, the mudpie loops.
  - **R12 Cleanup:** sorted, no note over the next of its voice past a tie, the event cap.
  - **Drums** run R0, R1 (their generator), R2, R5 (their own fills), R6 (choke lengths only),
    R7 (swing, push, accents), R4 as rolls and flams (subdivision, no doubling), R9 with the
    time operators only, R10, R11; no speed and no melody.
  - Every note carries where it came from (`LiveEvent::origin`: skeleton, fill, recursion,
    length split, operator; and whether its pitch is fixed) and its recursion group.
  - **As built (2026-10-23):** `crate::chain` (speed, recursion, fill, groove, cleanup), the
    bass generator split into its phases (`render_bar` the skeleton, `fill`, `lengths`,
    `melody`, `expression` on `LaneGenerator`), the operators in `ops.rs`, the form builder
    without rhythm operators (`FormSpec` lost its Density and Groove bands; the shared host
    form's byte for them is reserved). Gone with it: the bass's rhythm cells, ratchets, half
    beats, phrase-end turnaround, clip splits and syncopation, motif mode (no caller), the
    letter's triplet reflow on the bass, the half-speed centre (Normal is Rate 0).
    - Measured (10 seeds × 32 bars of generated forms, the bass at Density 0.5): Groove 0 puts
      18 % of the notes on the 1, 29 % on the other beats, 16 % on off 8ths, 28 % on off 16ths
      (half speed had 29 % and 29 % on the 1 and the beats, 7 % and 5 % off them); Groove 0.5
      moves 22 % off the 16ths and recurses 14 % of the notes; Groove 1 moves 42 % and recurses
      42 % (clips 35 %), the beats down to 14 %.
    - A held fill run is a figure (`flags::FIGURE`): the Length phase keeps it held. A clip
      plays as written at the middle of Stabs (its stab or held notes flip only off the middle).
    - The partial stabs, the fill and the speed share the finest grid the speed allows.
    - Splice's counter end: 6 % of the moving pairs land in parallel perfect intervals (2.7 %
      at half speed's 8ths); the downbeat's root, an anchor, keeps its place across a barline.
- **D58** **Neuro: the split sums flat, the chorus last** (2026-10-23, user). Research into why
  Neuro sounded degraded found the sub and the processed band meeting in antiphase at the
  130 Hz split (the band was highpassed a second time, the sub was not): with every stage off
  a sine came out −79 dB at 130 Hz, −8 at 110 and −9 at 150 Hz, at any amount. Now the dry
  band's part under that second highpass is added back and the sub takes the same split's
  allpass, plus the crossovers the band passes near it as allpasses (the drive's low one,
  moving with the register, and the low gap's two edges, moving with the note): from 60 to
  250 Hz a sine comes out within ±2 dB at a touch of Neuro and in full. The chorus (detune)
  moved from before the drive to the end of the chain, wide stereo (each side its own copy),
  at most 30 % wet: before the drive its copies distorted against each other, the most
  off-harmonic energy of any stage (a saw +8 dB). Open from the research: a mudpie's chords
  intermodulate in the drive (+13 dB off-harmonic on a three-note chord), the moving bells
  are direct-form biquads (TPT filters would modulate cleaner), the comb and phaser tune to
  the played key, which a mudpie's pitch may not be.
- **D59** **The bass's held notes and order** (2026-10-24, user: held and middle-long notes
  lacked punch, their release faded away too long; the Squash belongs after Heat).
  - **Release curve:** the sampler's release takes a shape (`release_curve`): 0 the
    exponential one (even in dB, to −60 dB), above 1 the level falls as 1 − xⁿ to silence.
    The bass uses n = 3: −1.2 dB at half the release, −6 dB at 80 %, then gone. Its release
    is shorter to match: the mudpie's designed one a 16th (was 3/32), a dotted 32nd at full
    Shape (was a quarter), the seeded spread at most a quarter (was a half).
  - **Attack curve:** the attack takes a shape too (`attack_curve`): 0 the quarter sine,
    above 1 the level rises as 1 − (1 − x)ⁿ. The bass uses n = 3: −6 dB at a fifth of the
    attack, −1.2 dB at half (the sine: −10 and −3 dB); the attack times stay (D52).
  - **Held punch:** held (non-stab) pitched notes take the stab drive at 0.4 of full (about
    8 dB, mostly taken back at the output) and half of that as the gain kick at their start,
    scaled by Accent like the stabs; their envelopes' pitch and cutoff stay as they were.
    Held notes keep half the mod envelope's cutoff sweep in the sustain (was 35 %).
  - **Shape 0 longer:** the shortening below Length's middle keeps a floor of 40 % undone
    (user: at Shape 0 the stabs were far too short, the top right). At Shape 0 a dotted-16th
    stab plays 26 ticks (was 20), a quarter 61 (was 38); the middle and top are unchanged.
    Only the note length moves: the stab pitch envelope reads the note as long as it was
    before the floor (`pitch_env_time`), and Shape's other parts (expression, stab share,
    filter bite, punch, release, attack) keep their mapping.
  - **Order:** the bass now runs Heat, then the Squash last, as the drums do (D53 had Heat
    last); the calibration follows.
- **D60** **Patterns in the DAW, patterns in the plugin** (2026-10-24, user, after a research
  into what MIDI and the hosts carry). The macro arrangement moves to the DAW, the patterns
  stay in the plugin.
  - **Slots.** A lane offers eight patterns, two per letter (A1, A2 .. D1, D2; `slots.rs`):
    the second is another seed of the letter (another clip pick in clip mode). Each slot
    plays its seeded pattern, a rerolled one (a per-slot seed), or the user's.
  - **The DAW's form.** The **Form source** parameter (automatable) picks who plays the
    form: the lane's own (INT) or the DAW's notes (EXT). In EXT a note on key 36 + slot
    (C1 .. G1 in most DAWs) plays that slot from its start for as long as it is held, its
    velocity the operator: 91–127 as is (a note the DAW draws at its default velocity plays
    the plain pattern), 1–9 ′, 10–19 r, 20–29 i, 30–39 b, 40–49 v, 50–59 h, 60–69 x, 70–79 q,
    80–90 a. One operator per note; the hosts cannot label velocity ranges (CLAP and VST3
    name keys only), so the editor and the exported file carry the names. A note loops its
    pattern's phrase (the seeded phrase or the user's window): a note-on does not say how
    long it will be held, so no operator spans the note. EXT plays on F1, moved by the
    **Transpose** parameter (semitones, ±24, automatable), which also moves the held note's
    root in INT. In INT the form keeps its Phase setting (continuous by default).
  - **Export.** `.mid ↓` writes the lane's own form over both passes as a type-0 Standard
    MIDI File (a note per segment on its slot's key, its operator as velocity, a marker
    naming it) to `~/Music/mesh vst/exports`, then switches to EXT, transposed to the root
    heard. Dragging out of the plugin window is a TODO: baseview takes drops only.
  - **The pattern editor.** The middle shows the visual or (EDIT) a slot's whole source: the
    clip, or four bars of the generator's skeleton and pitches (R1, R8, before the knobs'
    phases), the bass as a piano roll (rows in semitones over the root), the drums as a hit
    grid (rows the sample targets, by voice). The window that plays is bracketed and dragged;
    notes are added, moved, lengthened and deleted on a grid of 1/4 .. 1/32, dotted or
    triplet. The first edit turns the seed's source into the user's pattern, which then is
    the skeleton (R1, its pitches fixed like a clip's), so every phase after it still
    applies: speed, recursion, Density, Length, Groove, the operators, expression. An edit
    of A1 shows in every operator of A1; reroll and reset per slot.
  - **Undo** covers everything that plays (D26): the parameters, the pins, the phrase pins,
    the Splice host, the mod-out map and the patterns, and it lasts while the editor is
    closed.
  - **Look.** Chamfered chips for buttons, segmented knob bars, a transpose box (wheel or
    drag, a double click resets), the unison as one pad of three drag areas over its voices
    drawn as lines (their number, jagged and blurred by the detune, apart by the width), the
    form strip at the top of the footer (the form with its playhead, or in EXT the slot the
    DAW plays).
- **D61** **Cycle forms** (2026-10-26, user: "a question answer system like 2 beats A 6 beats
  B or 12 beats A then 4 beats B"). It replaces how D48 lays out a pass. D48 drew phrase
  lengths again per section, let plans place letters freely and kept B's length at 1×, 2× or
  ½× A's, so the A blocks came out long and short at random.
  - **Cycle.** A and B repeat as one cycle: A+B, or A+B+A (a letter twice splits its beats
    and goes on in its phrase). The cycle is 8, 16, 32 or 64 beats, so it fills 16 bars, and
    Shape tilts the length. The seed picks A's share: 1/4, 3/8, 1/2, 5/8 or 3/4 (2+6,
    3+5, 12+4). Contrast moved it at first; the user did not like that (2026-10-26).
  - **Drums.** Cycles of 32 or 64 beats, mostly A alone; at 30 % a cycle ends on a short
    B (an eighth of it).
  - **Motive.** A motive is its cycle repeated over 16, 32 or 64 bars, and the seed picks
    the length. A 64-bar pass plays two motives, X and Y: 16 bars as X Y X Y or X X Y Y,
    32 bars as X Y, 64 bars as X, with Y all through the second pass. Y is another motive:
    the letters' second patterns (A2, B2, D60, marked `"` in a form: `A"3`), its own split
    or X's flipped, the plan's Y operators, and the lane's other sound by Contrast.
  - **Plans** are characters: each picks the cycle shapes and how Y differs from X (the
    breakdown's Y plays half time, the switch-up's re-cuts, the space's truncates). X keeps
    to its `?` slots.
  - **Seams.** C, a short fill of up to 4 beats, ends each motive. D, up to 8 beats, ends
    the pass, each drawn from its own pool. A 16-bar seam inside a motive fills rarely and
    for at most 2 beats. The extra fills at 4-bar seams stay.
  - **Turnarounds.** The last bar of the main part before a motive's or the pass's seam
    plays an operator (x p F t u r j K on the bass, x F j z on the drums) at 50 .. 95 % by
    Variation, an inner seam at half; the pass's end now and then a push on top. The fill
    seed picks it per seam and pass.
  - **Second pass.** It draws its motives freely (again on a broken rule), then its recipe
    applies.
  - **One phrase per letter** and pattern for the whole form (`arrange::letter_phrase`; the
    DAW's slots loop the same one).
  - **Changes wait.** A spec change (a Contrast, Variation or Shape band) builds the new
    form, and the player switches to it when the next 16-bar section starts. A preview
    switches at once.
  - **Strip.** The form strip ticks each cycle's start.
  - **Level map** (`loader::level_gains`): near a stream's end a position is rated by what
    is left, not by a window wrapped into the start. Contrast's Morph had put B's notes in a
    library mudpie's faded tail, rated loud by the start, and B's bars, now whole, were 25 dB
    down.
  - **Truncation** leaves at most 2 beats of rest: a share of a long cycle part silenced
    bars.
  - The 4-bar repetition of the bass forms rose from about 0.70 to 0.80 (median of 30
    seeds, test bound now 0.85; the references measure 0.33 .. 0.63).
- **D62** **Listening round** (2026-10-26, user).
  - **Squash** at full kept too little sub and low: on a stab chord the sub fell 3.0 dB and
    the low 2.8, while the highs rose 7.5. The mids and highs keep their strong settings
    (a first try trimmed them; the user wanted only the bass stronger). The sub now
    compresses at 2.5:1 from −20 dB and the low at 5:1 from −26, each with +1 dB make-up.
    The mudpie's level match hears only the bands from the mids up, so the sub and low add
    on top; the drums' match still hears every band. The mudpie also comes out 3 dB louder
    at full (`squash::lift_db`, scaled by the amount), and the bass calibration keeps that
    lift. At full against the input: sub +3.6, low +2.1, mids +0.5, highs +8.0 dB. The
    drums keep their level.
  - **Shape's bottom** (user: 0 % too short, 40 % right). The Length at Shape 0 is 0.43,
    where 40 % was; the middle and the top stay.
  - **Contrast mirrors the Shape.** B's Length and stab share go towards and through the
    centre: at full Contrast to their mirror image (stabby A against sustained B at Shape 0,
    the other way at the top), at least half the range from the centre. This replaces
    D49's "at a knob's end B moves inward and less" for these two.
  - **Stab decay.** At Shape 0 the stabs decay twice as long as designed, falling to the
    designed decay at the top (`STAB_DECAY_STRETCH`).
  - **B's identity.** The drums' B and D play another style than A and C (seeded), at
    any Contrast. That breaks D49's "at 0 B plays A's settings" for the style alone.
- **Cost** (2026-10-23, clap-validator's parameter fuzz ran 48 s against its 45 s limit):
  the multiband distortion measures a pass's makeup again only when its drive moves 0.1 dB,
  the Squash makes its band settings only when the amount or the tempo changes, and the
  oversampling is 1× on the tape low bands, 2× on the mids and the slight tops (the drums'
  hard-clipped mids keep 4×). Neuro 10.5× → 15.4× realtime, Heat 27× → 61× (one core,
  optimised); the fuzz 43 s.
- **Snapshotted operators (D52, cont.):** which operator a slot plays is fixed by the fill seed per
    letter and kind of place (before a change, period end, repeat, other; the rhythm ones the
    same for every letter at one lean), the same in both passes and every rule attempt.
    Variation (melodic) and Density (rhythm) decide only how many of the slots play it, by
    the slot's place, so turning them up adds occurrences of the same operators.

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
- Drag the exported form (D60) out of the editor into the DAW: baseview has no drag source;
  it needs a drag per platform (XDND, OLE, NSDraggingSession).
- Name the slot keys in the DAW's piano roll (D60): CLAP's note-name extension and VST3's
  pitch names, which nice-plug does not offer yet (an upstream change, not a vendored copy);
  then check which hosts show them (Bitwig, Reaper).
- Slicing breaks on detected onsets (D29).
- Pitching tonal drums to the key (D7).
- Velocity as timbre (R3).
- Generic DAW key read via the VST3 chord and scale context (D11).

## 6. Parameters (D37)

Nine knobs at most: 7 shared, Spread and Slide (bass), Tone (drums), broad like Mesh live's lane controls. All
are automatable. The seed sets each knob's default center, so a new seed comes with its own
suggested settings. Turning a knob or automating it moves away from that center.

| Knob | Bass | Drums |
|---|---|---|
| **Density** | the skeleton's notes (about 2.5 a bar at the middle, D57), note length; clips (D50): as written at the middle, weak notes thinned below; from 0.55 the rests fill in clusters (D56), taking more of a held note's time the higher | hits, voices coming in (tops included), loop gating |
| **Contrast** | how far B moves from A (A plays the knobs): 0 the same settings, 1 the other side of the question and answer on every axis, in seeded directions; C and D around A and B (D49) | the same, with the drum axes |
| *Permute* (parameter, no knob since D49) | half-beat chunks of the bar swap places (seeded, downbeat stays; 0 = as generated) | the same |
| **Variation** | how far the operators go and how many slots play one; the mix of the Groove's moves (one way per letter at 0, each note its own at 1) and where a subdivision's window sits (D57); how many long notes loop (D43); scales the seeded accents, stab drive and filter sweep; how far B, C and D move Morph off A's (D52); Variation itself plays as set | rewrites, fills, ghost notes; scales the seeded accents and velocity range; the operators as on the bass |
| **Groove** | how often and how far notes move off the grid (pushes, pulls, 64th steps; rare and mild at the middle, often and up to an 8th at the top) and their lengths nudge, and how often notes recurse (a subdivision with the skeleton's own pattern, or a doubling; about 1 in 10 at the middle, 1 in 3 at the top, D57); the lane's own times in triplets, straight or dotted (D45) | the feel: swing of every off 16th, each voice pushed or laid back (snare late, hats and shakers early), the cymbals' accent shape, more and softer ghosts; rolls and flams (D57, rarer than the bass's recursion); the lane's own times in triplets, straight or dotted (D45) |
| **Shape** | stabby to sustained: the attack (5 ms .. 100 ms, D52), length (short stabs at 0, shortened at most 60 % of the way since D59, as written in the middle, held at full; Density's splits and fills then play as short subdivided hits at low Shape, 2026-10-21), expression, filter bite, the release (a 1/64 at 0, the designed 16th in the middle, a dotted 32nd at full, on a curve that holds the level and falls fast at the end, D59; held notes sustain at −1 dB, 2026-10-20, with a share of the stab drive); at full every note holds into the next (legato, no stabs) except the figures (held fill runs), which keep their lengths; stabs come in runs (D57), a held note's start or end sometimes a stab | tight to open: choke (at full Shape a retrigger fades over about 400 ms), loop hold, decay (it shortens every voice, the kick too, and lengthens none); the attack stays instant at every setting; a transient shaper lifts the attack a little below the middle and the sustain above it; from the middle up a release tail grows per voice: the snare up to a dotted 8th, the hats and cymbals half as long and a little weaker, the kick a faint dotted 32nd, the loops none; by the switch next to Shape or the sound seed: VOC (the hits' spectrum shapes noise, band by band), SPEC (spectral sustain, each bin fades from its peak), NOISE (three noise bands follow the hits) |
| **Morph** | mudpie scrub position | straight to chopped (break chop, rewrites) inside the Style; a ring modulator above 400 Hz (the lows stay clean) fades in from a fifth to at most 30 %, its seeded carrier rising an octave and turning from sine to square |
| **Spread** | width: octave lifts, jump size, notes above the home root spread in stereo (the sub stays centred), deeper filter LFO and envelope | — |
| **Slide** | how often notes slide (up to 45 % of eligible notes) and the glide time, 30 ms to 250 ms; also the glide of the full-Shape legato; liquid: up to 35 % more held notes tie, filter glide 4 to 160 ms, vibrato on held notes | — |
| **Tone** | — | the kit's register: low plays more kicks, snares and ghosts and fewer cymbals; high plays more hats, rides, shakers, top loop and break, sparser kicks and snares |
| **Squash** | the last stage: six-band up and down compression, mild to strong (D44), hard and smiley at the top | the same, much lighter |
| **Heat** | clip, then bitcrush, then saturation on the output: the clip threshold falls all the way up (0.95 to 0.25), saturation from a fifth up to +30 dB drive, the crush fades in from the middle (12 to 4 bits, up to 12 samples held), level-matched within a few dB | the same, but the transients stay (2026-10-20): the clip stays at 0.95 at every Heat (the lane comes in at its calibrated loudness) and the saturation runs in parallel, at most 35 % wet; Tone tilts the saturation (12 dB around 700 Hz): low Tone drives the lows, high Tone the highs |

A ninth, stepped and automatable parameter, **Style** (drums), picks the rhythm family
(D39).

The **settings panel** holds values that are seeded where sensible, saved with the project, and
not knobs: Rate, the Bass source (D50, next to Rate), Phrase, Phase, Scale, Bend range, Gain, and the mod-out assignment and
meters. **Form source** (INT, EXT) and **Transpose** (D60) sit in the right column and the top bar. The form's three seeds (D48) sit in the left column. Every parameter, the seeds and
settings included, is automatable (2026-10-21).


**Unison (Bass only, D38).** Each note plays a seeded number of detuned copies of the mudpie
voice. The unison pad at the right column's bottom (D60; the footer before) sets **Voices** (maximum, 1–8), **Detune** (cents) and
**Width** (stereo spread of the copies, 0 = mono, as for a reese): Detune and Width are the
centres each note draws its own value around, in a seeded range that Variation widens (the
per-note moves of Shape, Morph, Spread and Slide scale 0.05 to 1 over Variation, on a curve,
about 0.4 in the middle; the pattern knobs do not move per note, 2026-10-20); higher
notes lean more detuned and wider and vary more, low notes stay close and narrow (2026-10-20).
These are project state, not knobs. **Mono lows** (settings, on by default, not seeded) plays
the output below 80 Hz in mono.

The unison sum keeps equal loudness. Each copy starts at its own seeded phase (up to one period
of the note into the sample), so the copies do not add up in phase on the attack and then fall
back as they drift apart. The gain is worked out per note, from each pair of copies: how their
harmonics (eight, falling as a saw's) beat and line up over half a second, from their detune,
their phases and their pans; Stacked layers count the harmonics an octave or a fifth shares.
Every mode and voice count lands within 0.5 dB of a plain note (2026-10-20; it was 1/√N, which
made 7 copies at 25 cents much louder than 3). The **unison
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
    Voices and Detune limits. They sum at equal loudness (seeded start phases and a per-note
    gain from how the copies' harmonics beat, within 2 dB of a plain note over 1.5 s). The five modes are linear, soft, hard, stacked (octaves and fifths) and random.
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
