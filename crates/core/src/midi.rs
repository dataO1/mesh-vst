//! The form as DAW notes (D60): what the lane's own form plays, written as
//! a Standard MIDI File the DAW plays back in the external mode. Each
//! segment is one note on its slot's key ([`crate::slots::Slot::key`]), its
//! velocity the operator ([`crate::slots::VelOp`]), as long as the segment,
//! with a marker naming it.

use crate::form::{Play, BEATS_PER_BAR};
use crate::pattern::PatternLane;
use crate::slots::{Slot, VelOp};

/// One note of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormNote {
    /// Start and length in beats.
    pub beat: u32,
    pub beats: u32,
    pub slot: Slot,
    pub op: VelOp,
}

impl FormNote {
    /// "A1 r".
    pub fn label(&self) -> String {
        let mark = self.op.mark();
        if mark.is_empty() { self.slot.name().to_string() } else { format!("{} {mark}", self.slot.name()) }
    }
}

/// The notes of `lane`'s form over both passes: a note per segment, rests
/// left out.
pub fn form_notes(lane: &PatternLane) -> Vec<FormNote> {
    let form = lane.form();
    let total = 2 * form.beats() as u64;
    let (seed, l) = (lane.pattern_seed, lane.form_lane());
    let mut notes: Vec<FormNote> = Vec::new();
    let mut last: Option<(u8, bool)> = None;
    for beat in 0..total {
        let pos = form.pos_varied_at(beat, seed, l);
        let same = last == Some((pos.segment, pos.second));
        last = Some((pos.segment, pos.second));
        if !matches!(pos.play, Play::All) {
            continue;
        }
        let slot = Slot { letter: pos.letter, variant: pos.variant };
        match notes.last_mut() {
            Some(n) if same && n.slot == slot && n.beat + n.beats == beat as u32 => n.beats += 1,
            _ => notes.push(FormNote { beat: beat as u32, beats: 1, slot, op: lane.velocity_op(&pos) }),
        }
    }
    notes
}

/// Ticks per quarter note in the file.
const PPQ: u32 = 96;

fn vlq(mut v: u32, out: &mut Vec<u8>) {
    let mut buf = [0u8; 5];
    let mut i = 4;
    buf[i] = (v & 0x7f) as u8;
    v >>= 7;
    while v > 0 {
        i -= 1;
        buf[i] = 0x80 | (v & 0x7f) as u8;
        v >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

fn meta(delta: u32, kind: u8, data: &[u8], out: &mut Vec<u8>) {
    vlq(delta, out);
    out.extend_from_slice(&[0xff, kind]);
    vlq(data.len() as u32, out);
    out.extend_from_slice(data);
}

/// `notes` as a type-0 Standard MIDI File at `bpm`, its track named `name`.
pub fn write_smf(notes: &[FormNote], bpm: f64, name: &str) -> Vec<u8> {
    let mut track = Vec::new();
    meta(0, 0x03, name.as_bytes(), &mut track);
    let tempo = (60_000_000.0 / bpm.clamp(20.0, 999.0)).round() as u32;
    meta(0, 0x51, &tempo.to_be_bytes()[1..], &mut track);
    meta(0, 0x58, &[BEATS_PER_BAR as u8, 2, 24, 8], &mut track);
    // Note-ons, note-offs and markers by time; a note-off before a note-on
    // at the same tick.
    let mut events: Vec<(u32, u8, Vec<u8>)> = Vec::new();
    for n in notes {
        let on = n.beat * PPQ;
        let off = (n.beat + n.beats) * PPQ;
        let vel = n.op.velocity().unwrap_or(100).clamp(1, 127);
        events.push((on, 1, [&[0xffu8, 0x06][..], &[n.label().len() as u8], n.label().as_bytes()].concat()));
        events.push((on, 2, vec![0x90, n.slot.key(), vel]));
        events.push((off, 0, vec![0x80, n.slot.key(), 0]));
    }
    events.sort_by_key(|e| (e.0, e.1));
    let mut at = 0;
    for (t, _, bytes) in events {
        vlq(t - at, &mut track);
        track.extend_from_slice(&bytes);
        at = t;
    }
    meta(0, 0x2f, &[], &mut track);
    let mut out = Vec::with_capacity(track.len() + 22);
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(&(PPQ as u16).to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend_from_slice(&track);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::Letter;
    use crate::macros::FormSettings;

    fn lane() -> PatternLane {
        let mut l = PatternLane::bass(5, 5);
        l.set_form_spec(FormSettings::seeded(9).spec(false, 0.5, 0.5, 0.5));
        l
    }

    #[test]
    fn the_notes_cover_both_passes_in_order() {
        let l = lane();
        let notes = form_notes(&l);
        assert!(notes.len() > 4);
        let end = notes.last().map(|n| n.beat + n.beats).unwrap();
        assert!(end <= 2 * l.form().beats() as u32);
        for w in notes.windows(2) {
            assert!(w[0].beat + w[0].beats <= w[1].beat);
        }
        assert!(notes.iter().any(|n| n.slot.letter == Letter::A));
    }

    #[test]
    fn the_file_holds_a_note_per_segment() {
        let notes = vec![
            FormNote { beat: 0, beats: 16, slot: Slot::from_index(0), op: VelOp::Plain },
            FormNote { beat: 16, beats: 4, slot: Slot::from_index(2), op: VelOp::Op(crate::ops::Op::Retrograde) },
        ];
        let smf = write_smf(&notes, 174.0, "bass");
        assert_eq!(&smf[..4], b"MThd");
        assert_eq!(&smf[14..18], b"MTrk");
        let ons: Vec<(u8, u8)> = smf.windows(3).filter(|w| w[0] == 0x90).map(|w| (w[1], w[2])).collect();
        assert_eq!(ons, vec![(36, 100), (38, VelOp::Op(crate::ops::Op::Retrograde).velocity().unwrap())]);
        let track_len = u32::from_be_bytes(smf[18..22].try_into().unwrap()) as usize;
        assert_eq!(smf.len(), 22 + track_len);
    }

    #[test]
    fn vlq_encodes_like_the_spec() {
        for (v, want) in [(0u32, vec![0u8]), (0x7f, vec![0x7f]), (0x80, vec![0x81, 0x00]), (0x3fff, vec![0xff, 0x7f]), (0x4000, vec![0x81, 0x80, 0x00])] {
            let mut out = Vec::new();
            vlq(v, &mut out);
            assert_eq!(out, want, "{v:#x}");
        }
    }
}
