//! The shipped bass clips (D50) as compact tables, written on C minor. The
//! `write_clips` example turns them into the `.mid` files under
//! `assets/clips`, which the plugin embeds and reads like a user's clips.
//!
//! The format is the starter set's ([`crate::motif::starter`]): per bar the
//! 16-step rhythm (`x` a stab of half its span, `X` held for its span, `-`
//! continues, `.` rests) and the pitches of its onsets as scale degree names
//! over the root (`1 b3 5 b7 8`, `,` an octave lower, `'` higher).
//!
//! MUSICAL: first draft, tune by ear (user, 2026-10-21: distinct rhythmic
//! and melodic patterns, not too complex, since the knobs add the
//! complexity; music theory §2 riff construction, §3 rhythm at 174, §5.3
//! tension and release). Eight bars each, bars 4 and 8 their endings. A
//! states and asks (open endings on the 5th, the 7th, the 3rd), B answers
//! (home to the root or the 5th), C are milder fills and turnarounds, D the
//! aggressive transitions (stabs, octaves, the b2 and b5, chromatic pushes).

use crate::motif::starter::{Bar, StarterClip};

const fn clip(name: &'static str, bars: &'static [Bar]) -> StarterClip {
    StarterClip { name, root: 0, minor: true, bars }
}

/// Clips per letter shipped.
pub const PER_LETTER: usize = 10;

pub const A: [StarterClip; PER_LETTER] = [
    // 3-3-2 roller on the root and its octave.
    clip("roller", &[
        ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
        ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
        ("x--x--x-x--x--x-", "1 1 8 1 5 8"),
        ("x--x--x-x--x--x-", "1 1 8 1 b7 5"),
        ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
        ("x--x--x-x--x--x-", "1 1 8 1 5 8"),
        ("x--x--x-x--x--x-", "1 1 8 1 1 8"),
        ("x--x--x-x---x-x-", "1 1 8 b3 4 5"),
    ]),
    // Offbeat 8ths, the line rising at the ends.
    clip("offbeats", &[
        ("--x---x---x---x-", "1 1 8 1"),
        ("--x---x---x---x-", "1 1 8 1"),
        ("--x---x---x---x-", "1 1 8 5"),
        ("--x---x---x---x-", "1 b3 5 b7"),
        ("--x---x---x---x-", "1 1 8 1"),
        ("--x---x---x---x-", "1 1 8 1"),
        ("--x---x---x---x-", "1 1 8 5"),
        ("--x---x---x-x-x-", "1 1 8 b7 5"),
    ]),
    // A stab, its octave, a held note below.
    clip("stab-hold", &[
        ("x-----x---X-----", "1 8 ,b7"),
        ("x-----x---X-----", "1 8 ,b7"),
        ("x-----x---X-----", "1 8 ,5"),
        ("x-----x---X-----", "1 8 b3"),
        ("x-----x---X-----", "1 8 ,b7"),
        ("x-----x---X-----", "1 b3 ,b7"),
        ("x-----x---X-----", "1 8 ,5"),
        ("x-----x---X---x-", "1 8 5 b7"),
    ]),
    // A long root and a pickup into the next bar.
    clip("long-pickup", &[
        ("X-----------x-x-", "1 1 5"),
        ("X-----------x-x-", "1 b7 8"),
        ("X-----------x-x-", "1 1 5"),
        ("X-------x-x-x-x-", "1 5 b7 8 b7"),
        ("X-----------x-x-", "1 1 5"),
        ("X-----------x-x-", "1 b7 8"),
        ("X-----------x-x-", "1 1 5"),
        ("X-------x---x---", "1 b3 5"),
    ]),
    // Syncopated stabs around the backbeat.
    clip("syncopated", &[
        ("x--x-x---x-x--x-", "1 1 1 8 1 5"),
        ("x--x-x---x-x--x-", "1 1 1 8 1 5"),
        ("x--x-x---x-x--x-", "1 1 1 8 1 5"),
        ("x--x-x---x-x--x-", "1 1 1 8 b7 b3"),
        ("x--x-x---x-x--x-", "1 1 1 8 1 5"),
        ("x--x-x---x-x--x-", "1 1 1 8 b7 5"),
        ("x--x-x---x-x--x-", "1 1 1 8 1 5"),
        ("x--x-x---x--x---", "1 1 b3 4 5"),
    ]),
    // 3-3-3-3-4 on a pedal.
    clip("pedal", &[
        ("X--X--X--X--X---", "1 1 1 1 1"),
        ("X--X--X--X--X---", "1 1 1 1 1"),
        ("X--X--X--X--X---", "1 1 1 1 8"),
        ("X--X--X--X--X---", "1 1 1 ,b7 ,5"),
        ("X--X--X--X--X---", "1 1 1 1 1"),
        ("X--X--X--X--X---", "1 1 1 1 1"),
        ("X--X--X--X--X---", "1 1 1 1 8"),
        ("X--X--X--X--x-x-", "1 1 1 1 b3 4"),
    ]),
    // A call in the first half bar, space after it.
    clip("half-bar-call", &[
        ("x--x--x---------", "1 1 5"),
        ("x--x--x---x-----", "1 1 5 b7"),
        ("x--x--x---------", "1 1 5"),
        ("x--x--x---x-x---", "1 1 5 b7 8"),
        ("x--x--x---------", "1 1 5"),
        ("x--x--x---x-----", "1 1 5 b7"),
        ("x--x--x---------", "1 1 8"),
        ("x--x--x-x-x-x---", "1 1 5 5 b7 8"),
    ]),
    // Quarters bouncing on the octave.
    clip("octave-quarters", &[
        ("x---x---x---x---", "1 8 1 8"),
        ("x---x---x---x---", "1 8 1 8"),
        ("x---x---x---x---", "1 8 1 5"),
        ("x---x---x---x---", "1 8 b7 5"),
        ("x---x---x---x---", "1 8 1 8"),
        ("x---x---x---x---", "1 8 1 8"),
        ("x---x---x---x---", "1 8 1 5"),
        ("x---x---x-x-x-x-", "1 8 1 b3 4 5"),
    ]),
    // A chain of dotted 8ths across the bar.
    clip("dotted-chain", &[
        ("x--x--x--x--x---", "1 1 8 1 1"),
        ("x--x--x--x--x---", "1 1 8 1 5"),
        ("x--x--x--x--x---", "1 1 8 1 1"),
        ("x--x--x--x--x---", "1 1 8 b7 5"),
        ("x--x--x--x--x---", "1 1 8 1 1"),
        ("x--x--x--x--x---", "1 1 8 1 5"),
        ("x--x--x--x--x---", "1 1 8 1 1"),
        ("x--x--x--x--x-x-", "1 1 8 1 b3 5"),
    ]),
    // The kick's two-step: a stab, a pair, space.
    clip("two-step", &[
        ("x-----x-x-------", "1 1 8"),
        ("x-----x-x-----x-", "1 1 8 b7"),
        ("x-----x-x-------", "1 1 8"),
        ("x-----x-x---x-x-", "1 1 8 5 b7"),
        ("x-----x-x-------", "1 1 8"),
        ("x-----x-x-----x-", "1 1 8 5"),
        ("x-----x-x-------", "1 1 8"),
        ("x-----x-x---X---", "1 1 8 b3"),
    ]),
];

pub const B: [StarterClip; PER_LETTER] = [
    // 8ths that come home on the root.
    clip("home-8ths", &[
        ("x-x---x---x-x---", "1 1 5 b7 8"),
        ("x-x---x---x-x---", "1 1 5 b7 8"),
        ("x-x---x---x-x---", "1 1 5 b7 5"),
        ("x-x---x---x-X---", "b7 b7 5 4 1"),
        ("x-x---x---x-x---", "1 1 5 b7 8"),
        ("x-x---x---x-x---", "1 1 5 b7 8"),
        ("x-x---x---x-x---", "1 1 5 b7 5"),
        ("x-x---x---X-----", "5 5 b3 1"),
    ]),
    // A held root answered by three stabs.
    clip("held-answer", &[
        ("X-------x--x--x-", "1 5 b7 8"),
        ("X-------x--x--x-", "1 5 b7 8"),
        ("X-------x--x--x-", "1 5 b7 5"),
        ("X-------x--x--X-", "1 b7 5 1"),
        ("X-------x--x--x-", "1 5 b7 8"),
        ("X-------x--x--x-", "1 5 b7 8"),
        ("X-------x--x--x-", "1 5 b7 5"),
        ("X-------X-------", "5 1"),
    ]),
    // A pickup that lands on the half bar.
    clip("pickup-answer", &[
        ("....x-x-X-------", "5 b7 8"),
        ("....x-x-X-------", "5 b7 1"),
        ("....x-x-X-------", "5 b7 8"),
        ("....x-x-X---x-x-", "5 b7 8 5 1"),
        ("....x-x-X-------", "5 b7 8"),
        ("....x-x-X-------", "5 b7 1"),
        ("....x-x-X-------", "5 b7 8"),
        ("....x-x-X-------", "b3 2 1"),
    ]),
    // Root and 5th bouncing.
    clip("fifths", &[
        ("x---x-x-x---x-x-", "1 5 1 1 5 1"),
        ("x---x-x-x---x-x-", "1 5 1 1 5 1"),
        ("x---x-x-x---x-x-", "1 5 1 1 5 8"),
        ("x---x-x-x---X---", "1 5 1 b7 5"),
        ("x---x-x-x---x-x-", "1 5 1 1 5 1"),
        ("x---x-x-x---x-x-", "1 5 1 1 5 1"),
        ("x---x-x-x---x-x-", "1 5 1 1 5 8"),
        ("x---x-x-X-------", "1 5 b3 1"),
    ]),
    // Stab pairs, the 5th answering the root.
    clip("stab-pairs", &[
        ("x--x----x--x----", "1 1 5 5"),
        ("x--x----x--x----", "1 1 5 5"),
        ("x--x----x--x----", "1 1 b7 b7"),
        ("x--x----x--x--x-", "1 1 5 4 b3"),
        ("x--x----x--x----", "1 1 5 5"),
        ("x--x----x--x----", "1 1 5 5"),
        ("x--x----x--x----", "1 1 b7 b7"),
        ("x--x----X-------", "b3 2 1"),
    ]),
    // A falling line from the octave.
    clip("falling", &[
        ("X---x---X---x---", "8 b7 5 b7"),
        ("X---x---X---x---", "8 b7 5 b7"),
        ("X---x---X---x---", "8 b7 5 b3"),
        ("X---x---X---x---", "8 b7 5 1"),
        ("X---x---X---x---", "8 b7 5 b7"),
        ("X---x---X---x---", "8 b7 5 b7"),
        ("X---x---X---x---", "8 b7 5 b3"),
        ("X---x---X-------", "b7 5 1"),
    ]),
    // Rolling 8ths in two groups of three.
    clip("rolling", &[
        ("x-x-x---x-x-x---", "1 1 b3 1 1 5"),
        ("x-x-x---x-x-x---", "1 1 b3 1 1 5"),
        ("x-x-x---x-x-x---", "1 1 b3 1 1 b7"),
        ("x-x-x---x-x-X---", "1 1 b3 4 b3 1"),
        ("x-x-x---x-x-x---", "1 1 b3 1 1 5"),
        ("x-x-x---x-x-x---", "1 1 b3 1 1 5"),
        ("x-x-x---x-x-x---", "1 1 b3 1 1 b7"),
        ("x-x-x---X-------", "5 4 b3 1"),
    ]),
    // A late answer off the beat.
    clip("late", &[
        ("--x-x---X-------", "5 b7 1"),
        ("--x-x---X-------", "5 b7 1"),
        ("--x-x---X-------", "5 b7 8"),
        ("--x-x---X---x---", "5 b7 8 5"),
        ("--x-x---X-------", "5 b7 1"),
        ("--x-x---X-------", "5 b7 1"),
        ("--x-x---X-------", "5 b7 8"),
        ("--x-x---X-------", "b3 2 1"),
    ]),
    // Root, octave, root: wide and plain.
    clip("octave-answer", &[
        ("x-------x---x---", "1 8 1"),
        ("x-------x---x---", "1 8 1"),
        ("x-------x---x---", "1 8 5"),
        ("x-------x---x-x-", "1 8 5 1"),
        ("x-------x---x---", "1 8 1"),
        ("x-------x---x---", "1 8 1"),
        ("x-------x---x---", "1 8 5"),
        ("x-------X-------", "8 1"),
    ]),
    // Groups of three against the bar.
    clip("threes", &[
        ("x--x--x---x--x--", "1 b3 5 1 5"),
        ("x--x--x---x--x--", "1 b3 5 1 5"),
        ("x--x--x---x--x--", "1 b3 5 1 b7"),
        ("x--x--x---X-----", "1 b3 5 1"),
        ("x--x--x---x--x--", "1 b3 5 1 5"),
        ("x--x--x---x--x--", "1 b3 5 1 5"),
        ("x--x--x---x--x--", "1 b3 5 1 b7"),
        ("x--x--X---------", "5 b3 1"),
    ]),
];

pub const C: [StarterClip; PER_LETTER] = [
    // Up the scale to the 5th and back.
    clip("scale-run", &[
        ("x-x-x-x-X-------", "1 2 b3 4 5"),
        ("x-x-x-x-X-------", "5 4 b3 2 1"),
        ("x-x-x-x-X-------", "1 2 b3 4 5"),
        ("x-x-x-x-x-x-x-x-", "1 2 b3 4 5 b6 b7 8"),
        ("x-x-x-x-X-------", "1 2 b3 4 5"),
        ("x-x-x-x-X-------", "5 4 b3 2 1"),
        ("x-x-x-x-X-------", "1 2 b3 4 5"),
        ("x-x-x-x-X-------", "8 b7 b6 5 1"),
    ]),
    // The 7th turning round to the root.
    clip("turnaround", &[
        ("X-------x---x---", "1 b7 8"),
        ("X-------x---x---", "1 5 b7"),
        ("X-------x---x---", "1 b7 8"),
        ("X-------x-x-x---", "1 5 b7 8"),
        ("X-------x---x---", "1 b7 8"),
        ("X-------x---x---", "1 5 b7"),
        ("X-------x---x---", "1 b7 8"),
        ("X-------X-------", "b7 1"),
    ]),
    // The 5th and its upper neighbour.
    clip("neighbour", &[
        ("x---x---x---X---", "5 b6 5 1"),
        ("x---x---x---X---", "5 b6 5 1"),
        ("x---x---x---X---", "5 b6 5 b3"),
        ("x---x---x-x-X---", "5 b6 5 4 1"),
        ("x---x---x---X---", "5 b6 5 1"),
        ("x---x---x---X---", "5 b6 5 1"),
        ("x---x---x---X---", "5 b6 5 b3"),
        ("x---x---X-------", "5 4 1"),
    ]),
    // Down from the octave in quarters.
    clip("octave-drop", &[
        ("x---x---X-------", "8 5 1"),
        ("x---x---X-------", "8 b7 5"),
        ("x---x---X-------", "8 5 1"),
        ("x---x---x---X---", "8 b7 5 1"),
        ("x---x---X-------", "8 5 1"),
        ("x---x---X-------", "8 b7 5"),
        ("x---x---X-------", "8 5 1"),
        ("x---x---x---x---", "8 b7 5 b3"),
    ]),
    // Rest, then 8ths into the next bar.
    clip("pickup-8ths", &[
        ("--------x-x-x-x-", "1 b3 4 5"),
        ("--------x-x-x-x-", "5 4 b3 1"),
        ("--------x-x-x-x-", "1 b3 4 5"),
        ("--------x-x-x-x-", "5 b7 8 b7"),
        ("--------x-x-x-x-", "1 b3 4 5"),
        ("--------x-x-x-x-", "5 4 b3 1"),
        ("--------x-x-x-x-", "1 b3 4 5"),
        ("----x-x-x-x-x-x-", "1 b3 4 5 b7 8"),
    ]),
    // Quarters on the triad.
    clip("triad", &[
        ("x---x---x---x---", "b3 1 b3 5"),
        ("x---x---x---x---", "b3 1 b3 5"),
        ("x---x---x---x---", "b3 1 b3 b7"),
        ("x---x---x---X---", "b3 1 5 1"),
        ("x---x---x---x---", "b3 1 b3 5"),
        ("x---x---x---x---", "b3 1 b3 5"),
        ("x---x---x---x---", "b3 1 b3 b7"),
        ("x---x---X-------", "5 b3 1"),
    ]),
    // Held notes, half time.
    clip("half-time", &[
        ("X---------------", "1"),
        ("X-------X-------", "1 b7"),
        ("X---------------", "1"),
        ("X-------X-------", "5 b7"),
        ("X---------------", "1"),
        ("X-------X-------", "1 b7"),
        ("X---------------", "1"),
        ("X-------X---X---", "5 b7 1"),
    ]),
    // A step up and back, twice per bar.
    clip("steps", &[
        ("x-x-X---x-x-X---", "1 2 1 5 b6 5"),
        ("x-x-X---x-x-X---", "1 2 1 5 b6 5"),
        ("x-x-X---x-x-X---", "1 2 1 5 4 b3"),
        ("x-x-X---x-x-X---", "1 2 1 b3 2 1"),
        ("x-x-X---x-x-X---", "1 2 1 5 b6 5"),
        ("x-x-X---x-x-X---", "1 2 1 5 b6 5"),
        ("x-x-X---x-x-X---", "1 2 1 5 4 b3"),
        ("x-x-X---X-------", "1 2 1 5"),
    ]),
    // A walk down from the octave to the root.
    clip("walk-down", &[
        ("X---X---X---X---", "8 b7 b6 5"),
        ("X---X---X---X---", "4 b3 2 1"),
        ("X---X---X---X---", "8 b7 b6 5"),
        ("X---X---X---X---", "4 b3 4 5"),
        ("X---X---X---X---", "8 b7 b6 5"),
        ("X---X---X---X---", "4 b3 2 1"),
        ("X---X---X---X---", "8 b7 b6 5"),
        ("X---X---X-------", "b7 5 1"),
    ]),
    // Tom-like steps up to the octave.
    clip("toms", &[
        ("x--x--x-X-------", "1 5 8 1"),
        ("x--x--x-X-------", "1 5 8 5"),
        ("x--x--x-X-------", "1 5 8 1"),
        ("x--x--x-x--x--x-", "1 5 8 1 5 8"),
        ("x--x--x-X-------", "1 5 8 1"),
        ("x--x--x-X-------", "1 5 8 5"),
        ("x--x--x-X-------", "1 5 8 1"),
        ("x--x--x-x---X---", "1 5 8 b7 1"),
    ]),
];

pub const D: [StarterClip; PER_LETTER] = [
    // 8th stabs jumping the octave, a b2 on the backbeat.
    clip("octave-stabs", &[
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 1 8 1 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 b2 8 1 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 1 8 1 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 b2 8 1 8 b7 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 1 8 1 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 b2 8 1 8"),
        ("x-x-x-x-x-x-x-x-", "1 8 1 8 1 8 1 8"),
        ("x-x-x-x-X-------", "1 8 b2 8 1"),
    ]),
    // The chromatic climb b6 6 b7 7 into the root.
    clip("climb", &[
        ("X---X---X---X---", "b6 6 b7 7"),
        ("X---------------", "8"),
        ("X---X---X---X---", "b6 6 b7 7"),
        ("X-------x-x-x-x-", "8 1 8 1 8"),
        ("X---X---X---X---", "b6 6 b7 7"),
        ("X---------------", "8"),
        ("x-x-x-x-x-x-x-x-", "b6 b6 6 6 b7 b7 7 7"),
        ("X---------------", "8"),
    ]),
    // The root against its b5.
    clip("tritone", &[
        ("x--x--x-x--x--x-", "1 b5 1 b5 1 8"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 1 8"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 8 b7"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 4 b5"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 1 8"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 1 8"),
        ("x--x--x-x--x--x-", "1 b5 1 b5 8 b7"),
        ("x--x--x-X-------", "1 b5 4 1"),
    ]),
    // Quarters, then a climb of 8ths.
    clip("push", &[
        ("x---x---x-x-x-x-", "1 1 1 b3 4 b5"),
        ("x---x---x-x-x-x-", "1 1 1 b3 4 5"),
        ("x---x---x-x-x-x-", "1 1 1 b3 4 b5"),
        ("x-x-x-x-x-x-x-x-", "1 b3 4 b5 5 b7 7 8"),
        ("x---x---x-x-x-x-", "1 1 1 b3 4 b5"),
        ("x---x---x-x-x-x-", "1 1 1 b3 4 5"),
        ("x---x---x-x-x-x-", "1 1 1 b3 4 b5"),
        ("x-x-x-x-X-------", "5 b6 6 b7 8"),
    ]),
    // A jump up and a long drop below.
    clip("jump-drop", &[
        ("x-x-X-----------", "1 8 ,1"),
        ("x-x-X-----------", "1 8 ,b7"),
        ("x-x-X-----------", "1 8 ,1"),
        ("x-x-X-------x-x-", "1 8 ,1 b2 1"),
        ("x-x-X-----------", "1 8 ,1"),
        ("x-x-X-----------", "1 8 ,b7"),
        ("x-x-X-----------", "1 8 ,1"),
        ("x-x-x-x-X-------", "1 8 b7 b2 1"),
    ]),
    // The b2 leaning on the root.
    clip("b2-lean", &[
        ("x--x--x---x-x-x-", "1 b2 1 b2 1 8"),
        ("x--x--x---x-x-x-", "1 b2 1 b2 1 8"),
        ("x--x--x---x-x-x-", "1 b2 1 b2 b7 8"),
        ("x--x--x---x-x-x-", "1 b2 1 b5 4 b2"),
        ("x--x--x---x-x-x-", "1 b2 1 b2 1 8"),
        ("x--x--x---x-x-x-", "1 b2 1 b2 1 8"),
        ("x--x--x---x-x-x-", "1 b2 1 b2 b7 8"),
        ("x--x--x---X-----", "1 b2 1 1"),
    ]),
    // Stutters that double up.
    clip("stutter", &[
        ("x-x-x-x---------", "1 1 1 1"),
        ("x-x-x-x-x-x-x-x-", "1 1 1 1 8 8 8 8"),
        ("x-x-x-x---------", "1 1 1 1"),
        ("x-x-x-x-x-x-x-x-", "1 1 1 1 b2 b2 1 1"),
        ("x-x-x-x---------", "1 1 1 1"),
        ("x-x-x-x-x-x-x-x-", "1 1 1 1 8 8 8 8"),
        ("x-x-x-x---------", "1 1 1 1"),
        ("x-x-x-x-x-x-X---", "8 8 b7 b7 5 5 1"),
    ]),
    // One hit, a long hold, a stab low.
    clip("drop-stab", &[
        ("X-----------x---", "1 ,1"),
        ("X-----------x---", "1 ,b7"),
        ("X-----------x---", "1 ,1"),
        ("X-------x---x---", "1 b2 ,1"),
        ("X-----------x---", "1 ,1"),
        ("X-----------x---", "1 ,b7"),
        ("X-----------x---", "1 ,1"),
        ("X-------x-x-x-x-", "1 b2 1 b2 1"),
    ]),
    // Octaves climbing over the bar.
    clip("octave-climb", &[
        ("x---x---x---x---", "1 8 b3 b10"),
        ("x---x---x---x---", "4 11 5 12"),
        ("x---x---x---x---", "1 8 b3 b10"),
        ("x-x-x-x-x-x-x-x-", "4 11 b5 b12 5 12 b7 b14"),
        ("x---x---x---x---", "1 8 b3 b10"),
        ("x---x---x---x---", "4 11 5 12"),
        ("x---x---x---x---", "1 8 b3 b10"),
        ("x-x-x-x-X-------", "5 12 b7 b14 8"),
    ]),
    // Held dissonance resolving to the root.
    clip("dissonant", &[
        ("X-------X-------", "b5 1"),
        ("X-------X-------", "b2 1"),
        ("X-------X-------", "b5 1"),
        ("X-------x-x-x-x-", "b5 4 b3 b2 1"),
        ("X-------X-------", "b5 1"),
        ("X-------X-------", "b2 1"),
        ("X-------X-------", "b5 1"),
        ("x-x-x-x-X-------", "b2 b2 b5 b5 1"),
    ]),
];

/// The clips of letter `l` (0 = A .. 3 = D).
pub fn of(l: usize) -> &'static [StarterClip; PER_LETTER] {
    match l {
        0 => &A,
        1 => &B,
        2 => &C,
        _ => &D,
    }
}
