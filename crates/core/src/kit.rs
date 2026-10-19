//! Sample banks for the two modes: a drum kit on the generators' target
//! layout, and a mudpie for the bass (design D14, D16).

use basedrop::Shared;

use crate::gc::gc_handle;
use crate::generator::layout::{
    BACKBONE_BASE, BREAK_BASE, DEFAULT_BREAK_CHOKE, HATS_BASE, HATS_LEN, MAX_BREAK_SLICES, PERC_BASE, PERC_LEN, RIDE_BASE,
    RIDE_LEN, SHAKER_BASE, SHAKER_LEN,
};
use crate::generator::DrumLayout;
use crate::sampler::{SampleBank, SampleData, Slice, SliceType, SlotDef};

/// Choke group of the hats (open hats are cut by closed ones).
pub const HATS_CHOKE: u8 = 2;
/// Choke group of a mudpie: one mono voice that glides.
pub const MUDPIE_CHOKE: u8 = 1;

// MUSICAL: from Mesh live. Slice types of an amen-style break chopped in 16
// equal steps, used until breaks are sliced on their onsets (design todo).
const GRID_BREAK: [SliceType; 16] = {
    use SliceType::*;
    [Kick, Hat, Kick, Hat, Snare, Hat, Hat, Ghost, Hat, Ghost, Kick, Kick, Snare, Hat, Hat, Ghost]
};

/// The samples of one drum kit, by pool. Empty pools leave their voice out.
#[derive(Default, Clone)]
pub struct DrumKit {
    pub kick: Option<Shared<SampleData>>,
    pub snare: Option<Shared<SampleData>>,
    pub hats_closed: Vec<Shared<SampleData>>,
    pub hats_open: Vec<Shared<SampleData>>,
    pub rides: Vec<Shared<SampleData>>,
    pub crashes: Vec<Shared<SampleData>>,
    pub shakers: Vec<Shared<SampleData>>,
    pub perc: Vec<Shared<SampleData>>,
    /// A break loop, sliced (see [`grid_slices`]).
    pub break_loop: Option<Shared<SampleData>>,
}

impl DrumKit {
    /// The kit's target layout and sample bank.
    pub fn build(&self) -> (DrumLayout, SampleBank) {
        let mut bank = SampleBank::new();
        let mut layout = DrumLayout::empty();
        let hit = |s: &Shared<SampleData>| SlotDef::whole(s.clone()).unpitched();

        if let (Some(kick), Some(snare)) = (&self.kick, &self.snare) {
            bank.set(BACKBONE_BASE, hit(kick));
            bank.set(BACKBONE_BASE + 1, hit(snare));
            layout = layout.with_backbone(2);
        }

        let closed = self.hats_closed.iter().take(HATS_LEN as usize);
        let open = self.hats_open.iter().take(HATS_LEN as usize - closed.len());
        let (mut closed_mask, mut open_mask) = (0u32, 0u32);
        for (i, s) in closed.enumerate() {
            bank.set(HATS_BASE + i as u8, hit(s).with_choke(HATS_CHOKE));
            closed_mask |= 1 << i;
        }
        let first_open = closed_mask.count_ones() as usize;
        for (i, s) in open.enumerate() {
            bank.set(HATS_BASE + (first_open + i) as u8, hit(s).with_choke(HATS_CHOKE));
            open_mask |= 1 << (first_open + i);
        }
        if closed_mask | open_mask != 0 {
            layout = layout.with_hat_masks(closed_mask, open_mask, HATS_CHOKE);
        }

        let (mut rides, mut crashes) = (0u16, 0u16);
        for (i, s) in self.rides.iter().chain(&self.crashes).take(RIDE_LEN as usize).enumerate() {
            bank.set(RIDE_BASE + i as u8, hit(s));
            if i < self.rides.len() {
                rides |= 1 << i;
            } else {
                crashes |= 1 << i;
            }
        }
        layout = layout.with_rides(rides, crashes);

        for (i, s) in self.shakers.iter().take(SHAKER_LEN as usize).enumerate() {
            bank.set(SHAKER_BASE + i as u8, hit(s));
        }
        layout = layout.with_shakers(self.shakers.len());

        for (i, s) in self.perc.iter().take(PERC_LEN as usize).enumerate() {
            bank.set(PERC_BASE + i as u8, hit(s));
        }
        layout = layout.with_perc(self.perc.len());

        if let Some(brk) = &self.break_loop {
            let n = brk.slices.len().min(MAX_BREAK_SLICES);
            let mut types = [SliceType::Other; MAX_BREAK_SLICES];
            for (dst, s) in types.iter_mut().zip(&brk.slices[..n]) {
                *dst = s.kind;
            }
            for i in 0..n {
                if let Some(slot) = SlotDef::slice(brk.clone(), i) {
                    bank.set(BREAK_BASE + i as u8, slot.unpitched().with_choke(DEFAULT_BREAK_CHOKE));
                }
            }
            layout = layout.with_break(&types[..n], DEFAULT_BREAK_CHOKE);
        }
        (layout, bank)
    }
}

/// Chop a loop into `steps` equal slices typed like an amen break.
pub fn grid_slices(data: SampleData, steps: usize) -> SampleData {
    let steps = steps.clamp(1, MAX_BREAK_SLICES);
    let len = data.len();
    let slices = (0..steps)
        .map(|i| Slice { start: i * len / steps, end: (i + 1) * len / steps, kind: GRID_BREAK[i % GRID_BREAK.len()] })
        .collect();
    data.with_slices(slices)
}

/// A mudpie as a mono, gliding, streamed instrument on target 0: each note
/// starts at the position the generator picks.
pub fn mudpie_bank(sample: Shared<SampleData>) -> SampleBank {
    let mut bank = SampleBank::new();
    bank.set_mono(MUDPIE_CHOKE, true);
    bank.set(0, SlotDef::whole(sample).streamed().with_choke(MUDPIE_CHOKE));
    bank
}

/// Share a sample for the RT thread (dropped on the collector thread).
pub fn share(data: SampleData) -> Shared<SampleData> {
    Shared::new(&gc_handle(), data)
}
