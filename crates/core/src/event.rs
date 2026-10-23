//! Fixed-size, RT-safe event schema (design §4.5).

use crate::MAX_EVENTS_PER_BAR;

/// What an event does to its voice target.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventKind {
    /// Sampler one-shot / slice trigger.
    #[default]
    Trigger = 0,
    /// Pitched note start (sampler instrument or CLAP synth).
    NoteOn = 1,
    /// Pitched note end (matched by `note_id`).
    NoteOff = 2,
    /// Choke voices in the event's choke group.
    Choke = 3,
    /// Lane pitch bend: `cents` from the written pitch (sound packs).
    Bend = 4,
    /// Controller change: CC number in `target`, value 0..127 in `vel`
    /// (sound packs: filter, vibrato depth, the Tone CC, ...).
    Cc = 5,
}

/// Event flag bits.
pub mod flags {
    /// Play the sample backwards from `start`.
    pub const REVERSE: u8 = 1 << 0;
    /// Tie to the previous note (legato / glide over `glide`).
    pub const TIE: u8 = 1 << 1;
    /// Accented hit.
    pub const ACCENT: u8 = 1 << 2;
    /// Loop the slot while the gate is held.
    pub const LOOP: u8 = 1 << 3;
    /// A stab: no sustain (the note is its decay) and the lane's stab
    /// accent (saturated attack, deeper pitch move).
    pub const STAB: u8 = 1 << 4;
    /// A note of a rhythmic figure (a cell, a ratchet): legato from the
    /// lane's length leaves it as written.
    pub const FIGURE: u8 = 1 << 5;
    /// The note's loop section plays back and forth (else it repeats).
    pub const PING_PONG: u8 = 1 << 6;
}

/// Where a note came from in the render chain (D57), in
/// [`LiveEvent::origin`]'s low bits; [`origin::FIXED`] marks a written pitch.
pub mod origin {
    /// The skeleton (the generator's, or the clip's).
    pub const SKELETON: u8 = 0;
    /// Density's fill (R5).
    pub const FILL: u8 = 1;
    /// A subdivision's pattern (R4): never subdivided again.
    pub const RECURSION: u8 = 2;
    /// A stab the Length phase split off a held note (R6).
    pub const SPLIT: u8 = 3;
    /// An operator added it (R9).
    pub const OPERATOR: u8 = 4;
    /// The origin bits.
    pub const MASK: u8 = 0x0F;
    /// The pitch is written (a clip's): the melody keeps it.
    pub const FIXED: u8 = 0x80;
    /// A doubled note (R4): not doubled or subdivided again.
    pub const DOUBLED: u8 = 0x40;

    /// The origin of `o` without the marks.
    pub const fn of(o: u8) -> u8 {
        o & MASK
    }
}

/// One musical event inside a bar. `Copy`, fixed size, no heap.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LiveEvent {
    /// Position within the bar, 0..TICKS_PER_BAR.
    pub tick: u16,
    pub kind: EventKind,
    /// Bitset of [`flags`].
    pub flags: u8,
    /// Sampler slot / slice / drum sub-voice index.
    pub target: u8,
    /// 0 = none, else lane-local choke group.
    pub choke: u8,
    /// MIDI key, resolved from scale degree at render time.
    pub key: u8,
    /// Velocity 0..=127.
    pub vel: u8,
    /// Fine pitch in cents (also sample-root correction).
    pub cents: i16,
    /// Start offset within slot/slice, 0..=65535 maps to 0.0..1.0.
    pub start: u16,
    /// Gate length in ticks, 0 = one-shot (play to end).
    pub gate: u16,
    /// Voice identity for NoteOff / CLAP note_id.
    pub note_id: u16,
    /// 0 = lane default, else amp-decay override (log scale).
    pub decay: u8,
    /// 0 = lane default, else amp-attack override (log scale 1 ms .. 2 s,
    /// see [`attack_byte`](crate::sampler::attack_byte)).
    pub attack: u8,
    /// Bipolar offset to the lane filter cutoff.
    pub cutoff: i8,
    /// Glide time (log scale), used with `flags::TIE`.
    pub glide: u8,
    /// Bipolar offset to lane pan.
    pub pan: i8,
    /// The 16th step the generator wrote the event at, before swing, permute
    /// and speed move it: the step its per-note parameters were read at.
    pub step: u8,
    /// Mudpie loops (D43): where the note's loop section starts after the
    /// note's own start, and how long it is, in ticks; `loop_len` 0 = none.
    /// The voice plays into the section, then loops it, crossfaded at the
    /// seam (or back and forth with [`flags::PING_PONG`]).
    pub loop_at: u8,
    pub loop_len: u8,
    /// Which phase of the render chain made the note (D57, [`origin`]), and
    /// whether its pitch is fixed (a clip's written note).
    pub origin: u8,
    /// The recursion group the note belongs to (D57; 0 none).
    pub group: u8,
}

impl LiveEvent {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// One lane's rendered bar. Events are kept sorted by `tick`.
#[derive(Clone)]
pub struct BarEvents {
    len: u16,
    events: [LiveEvent; MAX_EVENTS_PER_BAR],
}

impl Default for BarEvents {
    fn default() -> Self {
        Self { len: 0, events: [LiveEvent::default(); MAX_EVENTS_PER_BAR] }
    }
}

impl BarEvents {
    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append an event. Returns `false` (and drops it) when full.
    pub fn push(&mut self, ev: LiveEvent) -> bool {
        if (self.len as usize) >= MAX_EVENTS_PER_BAR {
            return false;
        }
        self.events[self.len as usize] = ev;
        self.len += 1;
        true
    }

    /// Stable sort by tick (insertion sort: allocation-free, small N).
    pub fn sort(&mut self) {
        let evs = &mut self.events[..self.len as usize];
        for i in 1..evs.len() {
            let mut j = i;
            while j > 0 && evs[j - 1].tick > evs[j].tick {
                evs.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    pub fn as_slice(&self) -> &[LiveEvent] {
        &self.events[..self.len as usize]
    }

    /// Keep the events `keep(index, event)` accepts, in order.
    pub fn retain(&mut self, mut keep: impl FnMut(usize, &LiveEvent) -> bool) {
        let mut n = 0;
        for i in 0..self.len as usize {
            if keep(i, &self.events[i]) {
                self.events[n] = self.events[i];
                n += 1;
            }
        }
        self.len = n as _;
    }

    /// Keep the first `len` events.
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len as u16);
    }

    /// Edit events in place. Callers keep them sorted by tick.
    pub fn as_mut_slice(&mut self) -> &mut [LiveEvent] {
        &mut self.events[..self.len as usize]
    }
}
