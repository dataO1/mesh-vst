//! The render chain (design D57): a lane's bar is made by one fixed chain of
//! phases, each changing one thing and reading only what the phases before
//! it settled. Its result is the MIDI the sampler plays.
//!
//! | Phase | Changes | Where |
//! |---|---|---|
//! | R0 Resolve | each letter's knob values (Contrast), speed step, seeds | [`crate::pattern`], [`crate::contrast`] |
//! | R1 Skeleton | the rhythm, accents, base lengths, the base note; Density's reveal | the lane's generator ([`crate::generator::LaneGenerator::render_bar`]) |
//! | R2 Structure | A′, Permute | [`crate::pattern`] |
//! | R3 Speed | the Rate and the letter's speed step, the speed limit | [`speed`] |
//! | R4 Recursion | subdivision (depth 1) and doubling | [`recursion`] |
//! | R5 Density fill | clusters in the rests | [`fill`] (the drums fill in their generator) |
//! | R6 Length | stab or held, partial stabs, legato and staccato | the generator ([`crate::generator::LaneGenerator::lengths`]) |
//! | R7 Groove | onsets moved, lengths nudged | [`groove`] |
//! | R8 Melody | the pitches | the generator ([`crate::generator::LaneGenerator::melody`]) |
//! | R9 Operators | on the full notes of a span | [`crate::ops`] |
//! | R10 Splice | a follower's fit to its host | [`crate::pattern`], [`crate::splice`] |
//! | R11 Expression | velocity, slides, filter, attack, start, Morph, pan, loops | the generator ([`crate::generator::LaneGenerator::expression`]), [`crate::pattern`] |
//! | R12 Cleanup | sorted, overlaps, the cap | [`cleanup`] |
//!
//! Every draw is snapshotted: keyed by the pattern seed, the letter and the
//! note's written step, never by the bar, so a letter plays alike wherever
//! it plays. Every note carries where it came from
//! ([`crate::event::origin`]) and its recursion group.

pub(crate) mod fill;
pub(crate) mod groove;
pub(crate) mod recursion;
pub(crate) mod speed;

use crate::event::{flags, BarEvents, EventKind};
use crate::TICKS_PER_BAR;

/// R12: the bar sorted, no pitched note held over the next one (but into a
/// tie), nothing past the bar's end.
pub(crate) fn cleanup(out: &mut BarEvents) {
    out.sort();
    let ev = out.as_mut_slice();
    for i in 0..ev.len() {
        if ev[i].kind != EventKind::NoteOn {
            continue;
        }
        ev[i].gate = ev[i].gate.min(TICKS_PER_BAR.saturating_sub(ev[i].tick).max(1));
        let Some(n) = ev[i + 1..].iter().find(|x| x.kind == EventKind::NoteOn && x.tick > ev[i].tick).copied() else { continue };
        if !n.has(flags::TIE) && ev[i].tick + ev[i].gate > n.tick {
            ev[i].gate = (n.tick - ev[i].tick).max(1);
        }
    }
}
