//! The shipped bass clips (D50) as compact tables, written on C minor. The
//! `write_clips` example turns them into the `.mid` files under
//! `assets/clips`, which the plugin embeds and reads like a user's clips.
//!
//! Per bar the 16-step rhythm and the pitches of its onsets, as scale degree
//! names over the root (`1 b3 5 b7 8`, `,` an octave lower, `'` higher):
//!
//! - `X` held for its span, `x` a stab of half its span (at least a step),
//!   `A` and `a` the same accented, `g` a ghost (one step, soft), `-`
//!   continues the note before it, `.` rests. A note's span runs to the
//!   next onset or rest.
//! - The velocities follow the metric place (the downbeat loudest, the
//!   beats, the 8ths, the 16ths softest), held notes a little louder, the
//!   first note of bars 4 and 8 (the phrase points) louder, accents and
//!   ghosts on top: 72 .. 124, written into the MIDI.
//!
//! MUSICAL: first draft, tune by ear (user, 2026-10-21: distinct rhythmic
//! and melodic patterns, not too complex, since the knobs add the
//! complexity; long and short notes mixed, not all on the beat: notes on
//! the e and the a, offbeat 8ths, anticipations held across the beat,
//! 3-3-2 and dotted groupings, the downbeat mostly on the root; music
//! theory §2 riff construction, §3 rhythm at 174, §5.3 tension and
//! release). Eight bars each, bars 4 and 8 their endings. A states and asks
//! (open endings on the 5th, the 7th, the 3rd), B answers (home to the root
//! or the 5th), C are milder fills and turnarounds, D the aggressive
//! transitions (accents, octaves, the b2 and b5, chromatic pushes).

use crate::motif::starter::semitones;
use crate::motif::{Clip, RawNote};
use crate::{STEPS_PER_BAR, TICKS_PER_BAR, TICKS_PER_STEP};

/// One bar: the 16-step rhythm and the pitches of its onsets.
pub type Bar = (&'static str, &'static str);

/// A shipped clip's table.
#[derive(Debug, Clone, Copy)]
pub struct ClipTable {
    pub name: &'static str,
    pub bars: &'static [Bar],
}

const fn clip(name: &'static str, bars: &'static [Bar]) -> ClipTable {
    ClipTable { name, bars }
}

// MUSICAL: first draft, tune by ear. The written dynamics.
/// Velocity by metric place: the downbeat, the beats, the 8ths, the 16ths.
const VEL_PLACE: [u8; 4] = [112, 104, 96, 88];
/// Held notes, and the first note of bars 4 and 8, play this much louder.
const VEL_HELD: u8 = 4;
const VEL_PHRASE: u8 = 6;
/// Accents lift a note by this much, ghosts play at this velocity.
const VEL_ACCENT: u8 = 14;
const VEL_GHOST: u8 = 72;
const VEL_MAX: u8 = 124;
/// The root's MIDI key (C2).
const ROOT_KEY: i32 = 36;

impl ClipTable {
    /// The clip as notes, its root on C2.
    pub fn clip(&self) -> Clip {
        let mut notes = Vec::new();
        for (bar, (rhythm, pitches)) in self.bars.iter().enumerate() {
            let cells = rhythm.as_bytes();
            assert_eq!(cells.len(), STEPS_PER_BAR, "{}: bar {bar} needs 16 steps", self.name);
            let mut names = pitches.split_whitespace();
            let mut first = true;
            for (s, &c) in cells.iter().enumerate() {
                if !matches!(c, b'x' | b'X' | b'a' | b'A' | b'g') {
                    continue;
                }
                let span = 1 + cells[s + 1..].iter().take_while(|&&n| n == b'-').count();
                let steps = match c {
                    b'X' | b'A' => span,
                    b'g' => 1,
                    _ => span.div_ceil(2),
                };
                let place = match s {
                    0 => 0,
                    _ if s % 4 == 0 => 1,
                    _ if s % 2 == 0 => 2,
                    _ => 3,
                };
                let mut vel = VEL_PLACE[place];
                if matches!(c, b'X' | b'A') {
                    vel += VEL_HELD;
                }
                if first && (bar == 3 || bar == 7) {
                    vel += VEL_PHRASE;
                }
                vel = match c {
                    b'a' | b'A' => vel + VEL_ACCENT,
                    b'g' => VEL_GHOST,
                    _ => vel,
                };
                first = false;
                let name = names.next().unwrap_or_else(|| panic!("{}: bar {bar} has too few pitches", self.name));
                notes.push(RawNote {
                    start: (bar * TICKS_PER_BAR as usize + s * TICKS_PER_STEP as usize) as u32,
                    len: (steps * TICKS_PER_STEP as usize) as u32,
                    pitch: (ROOT_KEY + semitones(name)).clamp(0, 127) as u8,
                    vel: vel.min(VEL_MAX),
                });
            }
            assert!(names.next().is_none(), "{}: bar {bar} has too many pitches", self.name);
        }
        Clip { bars: self.bars.len() as u8, notes }
    }
}

/// Clips per letter shipped.
pub const PER_LETTER: usize = 10;

pub const A: [ClipTable; PER_LETTER] = [
    // A held root, its octave tied over beat 3, a ghosted 16th roll.
    clip("hold-and-roll", &[
        ("X-----X---x-xg-x", "1 8 1 5 5 1"),
        ("X-----X---x-xg-x", "1 8 1 b7 b7 1"),
        ("X-----X---x-xg-x", "1 8 1 5 5 1"),
        ("X-----X---x-xxxx", "1 8 1 5 b7 8 b7"),
        ("X-----X---x-xg-x", "1 8 1 5 5 1"),
        ("X-----X---x-xg-x", "1 8 1 b7 b7 1"),
        ("X-----X---x-xg-x", "1 8 1 5 5 1"),
        ("X-----X-------x-", "1 8 5"),
    ]),
    // Held dotted 8ths (3-3-3-3-2-2).
    clip("dotted-pulse", &[
        ("X--X--X--X--X-x-", "1 1 8 1 5 8"),
        ("X--X--X--X--X-x-", "1 1 8 1 b7 8"),
        ("X--X--X--X--X-x-", "1 1 8 1 5 8"),
        ("X--X--X--X--x-xx", "1 1 8 1 5 b7 8"),
        ("X--X--X--X--X-x-", "1 1 8 1 5 8"),
        ("X--X--X--X--X-x-", "1 1 8 1 b7 8"),
        ("X--X--X--X--X-x-", "1 1 8 1 5 8"),
        ("X--X--X-------x-", "1 1 8 b7"),
    ]),
    // Held offbeat 8ths, a 16th pickup into the last.
    clip("offbeat-glue", &[
        ("..X-..X-..X-.xX-", "1 1 8 1 1"),
        ("..X-..X-..X-.xX-", "1 1 8 5 8"),
        ("..X-..X-..X-.xX-", "1 1 8 1 1"),
        ("..X-..X-..X-xxX-", "1 1 8 5 b7 8"),
        ("..X-..X-..X-.xX-", "1 1 8 1 1"),
        ("..X-..X-..X-.xX-", "1 1 8 5 8"),
        ("..X-..X-..X-.xX-", "1 1 8 1 1"),
        ("X-X-..X-..X-X---", "1 1 8 5 b7"),
    ]),
    // One long root, a 16th pickup tied into the next bar's beat.
    clip("long-pickup", &[
        ("X----------x-xX-", "1 1 5 8"),
        ("X----------x-xX-", "1 1 b7 8"),
        ("X----------x-xX-", "1 1 5 8"),
        ("X-------x-xx-xa-", "1 5 b7 8 b7 5"),
        ("X----------x-xX-", "1 1 5 8"),
        ("X----------x-xX-", "1 1 b7 8"),
        ("X----------x-xX-", "1 1 5 8"),
        ("X-------X-------", "1 5"),
    ]),
    // An accented stab, the octave tied over beat 3.
    clip("stab-swell", &[
        ("a-----X-----x-x-", "1 8 1 5"),
        ("a-----X-----x-xx", "1 8 1 b3 5"),
        ("a-----X-----x-x-", "1 8 1 5"),
        ("a-----X---x-xxx-", "1 8 1 b3 4 5"),
        ("a-----X-----x-x-", "1 8 1 5"),
        ("a-----X-----x-xx", "1 8 1 b3 5"),
        ("a-----X-----x-x-", "1 8 1 5"),
        ("a-----X-------g-", "1 8 5"),
    ]),
    // Held 3-step groups, a 16th run at the turn.
    clip("triplet-feel", &[
        ("X--x--X--x--X-x-", "1 1 8 1 5 1"),
        ("X--x--X--x--X-x-", "1 1 8 1 b7 1"),
        ("X--x--X--x--X-x-", "1 1 8 1 5 1"),
        ("X--x--X--x--xxxx", "1 1 8 1 5 b7 8 b7"),
        ("X--x--X--x--X-x-", "1 1 8 1 5 1"),
        ("X--x--X--x--X-x-", "1 1 8 1 b7 1"),
        ("X--x--X--x--X-x-", "1 1 8 1 5 1"),
        ("X--x--X-----X---", "1 1 8 b7"),
    ]),
    // 16th pairs into notes held across the beat.
    clip("sixteenth-cells", &[
        ("xx.X---xx.X---x-", "1 1 8 1 1 5 b7"),
        ("xx.X---xx.X---x-", "1 1 8 1 1 b7 5"),
        ("xx.X---xx.X---x-", "1 1 8 1 1 5 b7"),
        ("xx.X---xx.X-xxxx", "1 1 8 1 1 5 b3 4 5 b7"),
        ("xx.X---xx.X---x-", "1 1 8 1 1 5 b7"),
        ("xx.X---xx.X---x-", "1 1 8 1 1 b7 5"),
        ("xx.X---xx.X---x-", "1 1 8 1 1 5 b7"),
        ("xx.X---X--------", "1 1 8 5"),
    ]),
    // A held root, the octave anticipating beat 3.
    clip("sustain-octave", &[
        ("X-----X---------", "1 8"),
        ("X-----X-----g-g-", "1 8 1 8"),
        ("X-----X---------", "1 8"),
        ("X-----X-----x-x-", "1 5 b7 8"),
        ("X-----X---------", "1 8"),
        ("X-----X-----g-g-", "1 8 1 8"),
        ("X-----X---------", "1 8"),
        ("X-----X-------x-", "1 b7 5"),
    ]),
    // A 3-3-2 roller pushed into a held end.
    clip("push-roller", &[
        ("x--x--x-x--x-xx-", "1 1 8 1 1 5 8"),
        ("x--x--x-x--x-xx-", "1 1 8 1 1 b7 8"),
        ("x--x--x-x--x-xx-", "1 1 8 1 1 5 8"),
        ("x--x--x-x--x-X--", "1 1 8 1 b7 5"),
        ("x--x--x-x--x-xx-", "1 1 8 1 1 5 8"),
        ("x--x--x-x--x-xx-", "1 1 8 1 1 b7 8"),
        ("x--x--x-x--x-xx-", "1 1 8 1 1 5 8"),
        ("x--x--x-X-------", "1 1 8 b7"),
    ]),
    // A short syncopated call and space, filled at the turn.
    clip("call-gap", &[
        ("X--x-x..........", "1 1 5"),
        ("X--x-x......x-x-", "1 1 5 b7 8"),
        ("X--x-x..........", "1 1 5"),
        ("X--x-x..x-x-xxX-", "1 1 5 b7 8 b7 5 b7"),
        ("X--x-x..........", "1 1 5"),
        ("X--x-x......x-x-", "1 1 5 b7 8"),
        ("X--x-x..........", "1 1 5"),
        ("X--x-x..X-------", "1 1 5 b3"),
    ]),
];

pub const B: [ClipTable; PER_LETTER] = [
    // Down from the octave, the root anticipating beat 4.
    clip("answer-hold", &[
        ("X-----x-x-X-----", "8 5 b3 1"),
        ("X-----x-x-X-----", "8 b7 5 1"),
        ("X-----x-x-X-----", "8 5 b3 1"),
        ("X-----x-x-x-x-X-", "8 5 b3 4 5 b7"),
        ("X-----x-x-X-----", "8 5 b3 1"),
        ("X-----x-x-X-----", "8 b7 5 1"),
        ("X-----x-x-X-----", "8 5 b3 1"),
        ("X-----x-x-X-----", "8 5 b3 1"),
    ]),
    // Held dotted notes falling to the root.
    clip("falling-dots", &[
        ("X--X--X--X--X---", "8 b7 5 b3 1"),
        ("X--X--X--X--X---", "8 5 b3 4 1"),
        ("X--X--X--X--X---", "8 b7 5 b3 1"),
        ("X--X--X--X--x-x-", "8 b7 5 4 b3 1"),
        ("X--X--X--X--X---", "8 b7 5 b3 1"),
        ("X--X--X--X--X---", "8 5 b3 4 1"),
        ("X--X--X--X--X---", "8 b7 5 b3 1"),
        ("X--X--X-----X---", "8 5 b3 1"),
    ]),
    // Root and octave on the e and the and, held across beats.
    clip("offbeat-answer", &[
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 8 1 5"),
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 b7 1 5"),
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 8 1 5"),
        ("x-X--xX-x-X-xxx-", "1 8 1 8 1 5 b3 4 5"),
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 8 1 5"),
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 b7 1 5"),
        ("x-X--xX-x-X--xX-", "1 8 1 8 1 8 1 5"),
        ("x-X--xX-X-------", "1 8 1 5 1"),
    ]),
    // A held root, a ghosted 16th roll up to the octave.
    clip("roll-home", &[
        ("X------x-gxgX---", "1 5 5 b7 b7 8"),
        ("X------x-gxgX---", "1 5 5 b3 b3 1"),
        ("X------x-gxgX---", "1 5 5 b7 b7 8"),
        ("X------x-gxgxgx-", "1 5 5 b7 b7 8 b7 5"),
        ("X------x-gxgX---", "1 5 5 b7 b7 8"),
        ("X------x-gxgX---", "1 5 5 b3 b3 1"),
        ("X------x-gxgX---", "1 5 5 b7 b7 8"),
        ("X------x-gxgX---", "1 5 5 b3 b3 1"),
    ]),
    // A late pickup into a root held across the half bar.
    clip("pickup-answer", &[
        ("......xX--------", "5 1"),
        ("......xX-----x--", "5 1 8"),
        ("......xX--------", "5 1"),
        ("......xX---x-x-x", "4 5 1 b3 5"),
        ("......xX--------", "5 1"),
        ("......xX-----x--", "5 1 8"),
        ("......xX--------", "5 1"),
        ("....x-xX--------", "b7 5 1"),
    ]),
    // A shuffled 3-3-2 with 16th pushes, home on the 5th.
    clip("shuffle-answer", &[
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 5"),
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 1"),
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 5"),
        ("X--x-xX--x-xx-x-", "1 1 8 1 1 8 b7 5"),
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 5"),
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 1"),
        ("X--x-xX--x-xX---", "1 1 8 1 1 8 5"),
        ("X--x-xX-----X---", "1 1 8 5 1"),
    ]),
    // An accented held root, stabs before the beats.
    clip("hold-stab-answer", &[
        ("A------x-----x--", "1 5 1"),
        ("A------x-----x--", "1 b7 1"),
        ("A------x-----x--", "1 5 1"),
        ("A------x---xxx--", "1 5 b7 5 4"),
        ("A------x-----x--", "1 5 1"),
        ("A------x-----x--", "1 b7 1"),
        ("A------x-----x--", "1 5 1"),
        ("A------x-x--X---", "1 5 b3 1"),
    ]),
    // Syncopated 8ths settling on a held root.
    clip("eighths-resolve", &[
        ("x--x-x-xX-------", "1 1 8 1 5"),
        ("x--x-x-xX-------", "1 1 b7 1 1"),
        ("x--x-x-xX-------", "1 1 8 1 5"),
        ("x--x-x-x-x-xx-x-", "1 1 8 1 5 4 b3 b7"),
        ("x--x-x-xX-------", "1 1 8 1 5"),
        ("x--x-x-xX-------", "1 1 b7 1 1"),
        ("x--x-x-xX-------", "1 1 8 1 5"),
        ("x--x-x-xX-------", "8 b7 5 b3 1"),
    ]),
    // Held notes around the backbeat, home at the end.
    clip("syncopated-answer", &[
        ("X-.x-.X-.x-.X---", "1 1 8 1 5"),
        ("X-.x-.X-.x-.X---", "1 1 8 1 1"),
        ("X-.x-.X-.x-.X---", "1 1 8 1 5"),
        ("X-.x-.X-.x-.xxx-", "1 1 8 1 b7 5 4"),
        ("X-.x-.X-.x-.X---", "1 1 8 1 5"),
        ("X-.x-.X-.x-.X---", "1 1 8 1 1"),
        ("X-.x-.X-.x-.X---", "1 1 8 1 5"),
        ("X-.x-.X-----X---", "1 1 5 1"),
    ]),
    // Held notes dropping below the root, off the beat.
    clip("low-answer", &[
        ("X--X---X--------", "1 ,b7 1"),
        ("X--X---X--------", "1 ,5 1"),
        ("X--X---X--------", "1 ,b7 1"),
        ("X--X---X----x-x-", "1 ,b7 ,5 ,b7 1"),
        ("X--X---X--------", "1 ,b7 1"),
        ("X--X---X--------", "1 ,5 1"),
        ("X--X---X--------", "1 ,b7 1"),
        ("X--X---X--------", "1 ,5 1"),
    ]),
];

pub const C: [ClipTable; PER_LETTER] = [
    // A held root and an offbeat 8th turnaround.
    clip("turnaround", &[
        ("X------x-x-x-x--", "1 5 b7 8 b7"),
        ("X------x-x-x-x--", "1 5 b7 8 5"),
        ("X------x-x-x-x--", "1 5 b7 8 b7"),
        ("X------xxxxx-x--", "1 5 5 b7 b7 8 5"),
        ("X------x-x-x-x--", "1 5 b7 8 b7"),
        ("X------x-x-x-x--", "1 5 b7 8 5"),
        ("X------x-x-x-x--", "1 5 b7 8 b7"),
        ("X------x-x-X----", "1 b3 4 5"),
    ]),
    // Root and octave with ghosted 8ths between.
    clip("ghost-bounce", &[
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 5 5"),
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 b7 b7"),
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 5 5"),
        ("X-g-x-g-X-g-xxxx", "1 1 8 8 1 1 5 b7 8 b7"),
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 5 5"),
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 b7 b7"),
        ("X-g-x-g-X-g-x-g-", "1 1 8 8 1 1 5 5"),
        ("X-g-x-g-X-------", "1 1 8 8 1"),
    ]),
    // Held notes, half the bar left open.
    clip("half-hold", &[
        ("X-----X-........", "1 5"),
        ("X-----X-....x-x-", "1 5 8 5"),
        ("X-----X-........", "1 5"),
        ("X-----X-x-x-x-x-", "1 5 b3 4 5 b7"),
        ("X-----X-........", "1 5"),
        ("X-----X-....x-x-", "1 5 8 5"),
        ("X-----X-........", "1 5"),
        ("X-----X---------", "1 5"),
    ]),
    // A climb on the e and the a.
    clip("rising-sixteenths", &[
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 4 4 5 5 b7 b7"),
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 5 5 8 8 5 5"),
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 4 4 5 5 b7 b7"),
        ("x-xx-x-xx-x-X---", "5 5 b7 b7 8 8 5 5"),
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 4 4 5 5 b7 b7"),
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 5 5 8 8 5 5"),
        ("x-xx-x-xx-x-xx-x", "1 1 b3 b3 4 4 5 5 b7 b7"),
        ("x-xx-x-xX-------", "1 b3 4 5 4 1"),
    ]),
    // Dotted held notes, 8ths filling the bar's end.
    clip("dotted-fill", &[
        ("X--X--X-x-x-x-x-", "1 1 8 1 5 b7 8"),
        ("X--X--X-x-x-x-x-", "1 1 8 1 b3 4 5"),
        ("X--X--X-x-x-x-x-", "1 1 8 1 5 b7 8"),
        ("X--X--X-xxxxx-x-", "1 1 8 1 b3 4 5 b7 8"),
        ("X--X--X-x-x-x-x-", "1 1 8 1 5 b7 8"),
        ("X--X--X-x-x-x-x-", "1 1 8 1 b3 4 5"),
        ("X--X--X-x-x-x-x-", "1 1 8 1 5 b7 8"),
        ("X--X--X-X-------", "1 1 8 5"),
    ]),
    // A held root and a 16th run.
    clip("sixteenth-run", &[
        ("X-------xxxxxxx-", "1 1 b3 4 5 b7 5 4"),
        ("X-------xxxxxxx-", "1 1 b3 4 5 4 b3 1"),
        ("X-------xxxxxxx-", "1 1 b3 4 5 b7 5 4"),
        ("X-------xxxxxxxx", "1 5 b7 8 b7 5 4 b3 1"),
        ("X-------xxxxxxx-", "1 1 b3 4 5 b7 5 4"),
        ("X-------xxxxxxx-", "1 1 b3 4 5 4 b3 1"),
        ("X-------xxxxxxx-", "1 1 b3 4 5 b7 5 4"),
        ("X-------x-x-X---", "1 4 5 1"),
    ]),
    // Held dotted notes with rests, a run at the turn.
    clip("stop-start", &[
        ("X--x....X--x....", "1 5 1 8"),
        ("X--x....X--x..x-", "1 5 1 8 5"),
        ("X--x....X--x....", "1 5 1 8"),
        ("X--x....X--xxxxx", "1 5 1 1 b3 4 5 b7"),
        ("X--x....X--x....", "1 5 1 8"),
        ("X--x....X--x..x-", "1 5 1 8 5"),
        ("X--x....X--x....", "1 5 1 8"),
        ("X--x....X-------", "1 5 1"),
    ]),
    // Soft pulses off the beat.
    clip("soft-pulse", &[
        ("x--x---x--x---x-", "1 8 1 5 1"),
        ("x--x---x--x---x-", "1 8 1 b7 1"),
        ("x--x---x--x---x-", "1 8 1 5 1"),
        ("x--x---x--x-x-x-", "1 8 b7 5 b7 8"),
        ("x--x---x--x---x-", "1 8 1 5 1"),
        ("x--x---x--x---x-", "1 8 1 b7 1"),
        ("x--x---x--x---x-", "1 8 1 5 1"),
        ("x--x---X--------", "1 5 1"),
    ]),
    // A held 3-3-2 with a turn.
    clip("swing-turn", &[
        ("X--x--x-X--x--x-", "1 1 5 1 1 b7"),
        ("X--x--x-X--x--x-", "1 1 5 1 1 5"),
        ("X--x--x-X--x--x-", "1 1 5 1 1 b7"),
        ("X--x--x-X--xxxx-", "1 1 5 1 b3 4 5 b7"),
        ("X--x--x-X--x--x-", "1 1 5 1 1 b7"),
        ("X--x--x-X--x--x-", "1 1 5 1 1 5"),
        ("X--x--x-X--x--x-", "1 1 5 1 1 b7"),
        ("X--x--x-X-------", "1 1 5 1"),
    ]),
    // Held notes walking up, before the beats.
    clip("octave-walk", &[
        ("X------X--------", "1 8"),
        ("X--X---X--X-----", "1 b3 4 5"),
        ("X------X--------", "1 8"),
        ("X--X---X--x-x-x-", "1 b3 4 5 b7 8"),
        ("X------X--------", "1 8"),
        ("X--X---X--X-----", "1 b3 4 5"),
        ("X------X--------", "1 8"),
        ("X--X---X--------", "8 5 1"),
    ]),
];

pub const D: [ClipTable; PER_LETTER] = [
    // Accented root and octave stabs in 8ths.
    clip("octave-stabs", &[
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 1 8 1 8"),
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 b7 8 5 8"),
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 1 8 1 8"),
        ("a-x-a-x-axaxaxax", "1 8 1 8 1 8 b7 8 5 8 b7 8"),
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 1 8 1 8"),
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 b7 8 5 8"),
        ("a-x-a-x-a-x-a-x-", "1 8 1 8 1 8 1 8"),
        ("a-x-a-x-A-------", "1 8 1 8 1"),
    ]),
    // A held root pushed by the b5 off the beat.
    clip("b5-push", &[
        ("A------a-a-a-a--", "1 b5 b5 5 5"),
        ("A------a-a-a-a--", "1 b5 b5 b7 b7"),
        ("A------a-a-a-a--", "1 b5 b5 5 5"),
        ("A------axaxaxax-", "1 b5 b5 5 5 b7 b7 8 8"),
        ("A------a-a-a-a--", "1 b5 b5 5 5"),
        ("A------a-a-a-a--", "1 b5 b5 b7 b7"),
        ("A------a-a-a-a--", "1 b5 b5 5 5"),
        ("A------A--------", "1 b5"),
    ]),
    // Syncopated accents, then a chromatic climb.
    clip("chromatic-climb", &[
        ("a--a---a--a---a-", "1 1 b2 1 1"),
        ("a--a---a--a---a-", "1 1 b2 b2 1"),
        ("a--a---a--a---a-", "1 1 b2 1 1"),
        ("a-a-a-a-a-a-a-a-", "1 b2 b3 4 b5 5 b6 b7"),
        ("a--a---a--a---a-", "1 1 b2 1 1"),
        ("a--a---a--a---a-", "1 1 b2 b2 1"),
        ("a--a---a--a---a-", "1 1 b2 1 1"),
        ("a-a-a-a-A-------", "5 b6 b7 7 8"),
    ]),
    // Ghosted 16ths with accents on the beats.
    clip("machine-16ths", &[
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 5 5 5 5"),
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 b7 b7 b7 b7"),
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 5 5 5 5"),
        ("agxgagxgagxgaxax", "1 1 1 1 8 8 8 8 1 1 1 1 5 b7 8 b7"),
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 5 5 5 5"),
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 b7 b7 b7 b7"),
        ("agxgagxgagxgagxg", "1 1 1 1 8 8 8 8 1 1 1 1 5 5 5 5"),
        ("agxgagxgA-------", "1 1 1 1 8 8 8 8 1"),
    ]),
    // One long hit and a low stab before the beat.
    clip("drop-stab", &[
        ("A----------a----", "1 ,1"),
        ("A----------a----", "1 ,b7"),
        ("A----------a----", "1 ,1"),
        ("A-------a--a-a--", "1 b2 ,1 ,1"),
        ("A----------a----", "1 ,1"),
        ("A----------a----", "1 ,b7"),
        ("A----------a----", "1 ,1"),
        ("A------axaxaxax-", "1 b2 1 b2 1 b5 5 b7 8"),
    ]),
    // Accented dotted 8ths.
    clip("dotted-hammer", &[
        ("a--a--a--a--a-a-", "1 1 8 1 1 8"),
        ("a--a--a--a--a-a-", "1 1 8 1 b7 8"),
        ("a--a--a--a--a-a-", "1 1 8 1 1 8"),
        ("a--a--a--a--axax", "1 1 8 1 1 b7 8 b7"),
        ("a--a--a--a--a-a-", "1 1 8 1 1 8"),
        ("a--a--a--a--a-a-", "1 1 8 1 b7 8"),
        ("a--a--a--a--a-a-", "1 1 8 1 1 8"),
        ("a--a--a-A-------", "1 1 8 1"),
    ]),
    // Accented octaves off the beat.
    clip("offbeat-octaves", &[
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 1 8 1 8"),
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 b7 8 5 8"),
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 1 8 1 8"),
        ("x-a-x-a-x-a-axax", "1 8 1 8 1 8 b7 b14 5 12"),
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 1 8 1 8"),
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 b7 8 5 8"),
        ("x-a-x-a-x-a-x-a-", "1 8 1 8 1 8 1 8"),
        ("x-a-x-a-A-------", "1 8 1 8 1"),
    ]),
    // Held dissonance resolving to the root.
    clip("dissonant-hold", &[
        ("A------A--------", "b5 1"),
        ("A------A--------", "b2 1"),
        ("A------A--------", "b5 1"),
        ("A------axaxaxax-", "b5 4 b3 b2 1 b2 1 b2 1"),
        ("A------A--------", "b5 1"),
        ("A------A--------", "b2 1"),
        ("A------A--------", "b5 1"),
        ("axaxaxaxA-------", "b2 b2 b5 b5 b2 b2 b5 b5 1"),
    ]),
    // Accented 3-step stabs.
    clip("triplet-stabs", &[
        ("a--a--a--a--a---", "1 8 1 8 1"),
        ("a--a--a--a--a---", "1 8 1 8 b7"),
        ("a--a--a--a--a---", "1 8 1 8 1"),
        ("a--a--a--a--axax", "1 8 1 8 b7 8 5 8"),
        ("a--a--a--a--a---", "1 8 1 8 1"),
        ("a--a--a--a--a---", "1 8 1 8 b7"),
        ("a--a--a--a--a---", "1 8 1 8 1"),
        ("a--a--a--a--A---", "1 8 1 8 1"),
    ]),
    // Rests, two stabs on the a and a slam.
    clip("rest-slam", &[
        ("...a-a-....A----", "1 1 1"),
        ("...a-a-....A----", "1 1 ,b7"),
        ("...a-a-....A----", "1 1 1"),
        ("...a-a-....axax-", "1 1 b2 b2 1 b5"),
        ("...a-a-....A----", "1 1 1"),
        ("...a-a-....A----", "1 1 ,b7"),
        ("...a-a-....A----", "1 1 1"),
        ("a-a-a-a-A-------", "1 b2 b3 b5 1"),
    ]),
];

/// The clips of letter `l` (0 = A .. 3 = D).
pub fn of(l: usize) -> &'static [ClipTable; PER_LETTER] {
    match l {
        0 => &A,
        1 => &B,
        2 => &C,
        _ => &D,
    }
}
