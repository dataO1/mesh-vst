//! Plays a [`PatternLane`] through the sampler in time with the host.
//!
//! The host's song position decides which bar and tick a block covers, so a
//! loop, a jump or an offline bounce always fires the same events at the
//! same samples (design D6). Sound plays only while a note is held (D7): the
//! held note's pitch class is the root, its velocity scales the hits. A
//! note-off closes the gate on the next 16th step, and sounding samples ring
//! out (D9). In [`Phase::Retrigger`] bar 0 starts at the note-on (D8).
//!
//! RT contract: after [`Player::new`] nothing here allocates, locks or blocks.

use crate::event::{EventKind, LiveEvent};
use crate::generator::layout::DrumVoice;
use crate::key::MusicalKey;
use crate::pattern::{PatternLane, RenderedBar, Scratch};
use crate::role::Role;
use crate::sampler::Sampler;
use crate::types::StereoSample;
use crate::{TICKS_PER_BAR, TICKS_PER_STEP};

/// Event ticks per quarter note.
pub const TICKS_PER_BEAT: f64 = TICKS_PER_BAR as f64 / 4.0;
/// A block starting this close (in ticks) to where the last one ended
/// continues it; further away is a jump.
const CONTINUE_TICKS: f64 = 1.0;

/// The host's transport at the start of a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    pub playing: bool,
    /// Song position in quarter notes.
    pub beat: f64,
    pub bpm: f64,
}

/// Where bar 0 of the pattern is (design D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Locked to the song: the bar at song bar N is pattern bar N.
    #[default]
    Continuous,
    /// The pattern restarts at each note-on.
    Retrigger,
}

#[derive(Debug, Clone, Copy)]
struct Gate {
    key: u8,
    /// Velocity scale 0..1.
    vel: f32,
    /// Pattern bar 0, in song ticks.
    origin: f64,
    /// Events at or after this song tick are not fired (a note-off).
    close_at: f64,
}

/// One lane's pattern and sampler.
pub struct Player {
    lane: PatternLane,
    sampler: Sampler,
    sample_rate: f64,
    pub phase: Phase,
    /// Scale family of the root: minor or major.
    pub minor: bool,
    gate: Option<Gate>,
    /// Song tick the last block ended at.
    until: Option<f64>,
    bar: RenderedBar,
    scratch: Scratch,
    /// What `bar` was rendered for: pattern bar, lane version, root key.
    cached: Option<(u64, u64, u8)>,
    version: u64,
}

impl Player {
    pub fn new(lane: PatternLane, sampler: Sampler) -> Self {
        let sample_rate = sampler.sample_rate() as f64;
        Self {
            lane,
            sampler,
            sample_rate,
            phase: Phase::default(),
            minor: true,
            gate: None,
            until: None,
            bar: RenderedBar::default(),
            scratch: Scratch::default(),
            cached: None,
            version: 0,
        }
    }

    pub fn lane(&self) -> &PatternLane {
        &self.lane
    }

    /// Change the lane (seeds, parameters, form). The current bar renders
    /// again on the next block.
    pub fn lane_mut(&mut self) -> &mut PatternLane {
        self.version = self.version.wrapping_add(1);
        &mut self.lane
    }

    pub fn sampler(&self) -> &Sampler {
        &self.sampler
    }

    pub fn sampler_mut(&mut self) -> &mut Sampler {
        &mut self.sampler
    }

    /// The pattern bar playing, if any (the last block's).
    pub fn rendered(&self) -> Option<(u64, &RenderedBar)> {
        self.cached.map(|(bar, _, _)| (bar, &self.bar))
    }

    /// A note starts at song tick `tick` with velocity 0..1. The newest held
    /// note wins (D10).
    pub fn note_on(&mut self, key: u8, vel: f32, tick: f64) {
        let origin = match (self.phase, self.gate) {
            (Phase::Continuous, _) => 0.0,
            // Legato into a new note keeps the running pattern.
            (Phase::Retrigger, Some(g)) if g.close_at.is_infinite() => g.origin,
            (Phase::Retrigger, _) => (tick / TICKS_PER_STEP as f64).round() * TICKS_PER_STEP as f64,
        };
        self.gate = Some(Gate { key, vel: vel.clamp(0.0, 1.0), origin, close_at: f64::INFINITY });
    }

    /// The held note `key` ends at song tick `tick`: the gate closes on the
    /// next 16th step.
    pub fn note_off(&mut self, key: u8, tick: f64) {
        if let Some(g) = self.gate.as_mut().filter(|g| g.key == key) {
            let step = TICKS_PER_STEP as f64;
            let local = tick - g.origin;
            g.close_at = g.origin + (local / step).ceil() * step;
        }
    }

    /// Whether a note holds the gate open.
    pub fn is_gated(&self) -> bool {
        self.gate.is_some_and(|g| g.close_at.is_infinite())
    }

    /// Render one block into `out` (overwritten).
    pub fn process(&mut self, out: &mut [StereoSample], t: &Transport) {
        let n = out.len();
        let bpm = t.bpm.max(1.0);
        let samples_per_tick = self.sample_rate * 60.0 / (bpm * TICKS_PER_BEAT);
        if t.playing && n > 0 {
            let t0 = t.beat * TICKS_PER_BEAT;
            let t1 = t0 + n as f64 / samples_per_tick;
            let start = match self.until {
                Some(u) if (u - t0).abs() <= CONTINUE_TICKS => u,
                _ => t0,
            };
            self.fire_range(start, t1, t0, samples_per_tick, n);
            self.until = Some(t1);
            if self.gate.is_some_and(|g| g.close_at <= t1) {
                self.gate = None;
            }
        } else {
            self.until = None;
        }
        self.sampler.process(out, samples_per_tick);
    }

    /// Fire the events of song ticks `from..to` (block starts at `t0`).
    fn fire_range(&mut self, from: f64, to: f64, t0: f64, samples_per_tick: f64, frames: usize) {
        let Some(gate) = self.gate else { return };
        let to = to.min(gate.close_at);
        if to <= from {
            return;
        }
        let bar_ticks = TICKS_PER_BAR as f64;
        let first = ((from - gate.origin) / bar_ticks).floor().max(0.0) as u64;
        let last = ((to - gate.origin) / bar_ticks).floor().max(0.0) as u64;
        let key = MusicalKey::new(gate.key % 12, self.minor);
        for bar in first..=last {
            let want = (bar, self.version, key.root);
            if self.cached != Some(want) {
                self.lane.render(bar, key, &mut self.bar, &mut self.scratch);
                self.cached = Some(want);
            }
            let bar_start = gate.origin + bar as f64 * bar_ticks;
            let drums = self.lane.role() == Role::Drums;
            for ev in self.bar.events.as_slice() {
                let at = bar_start + ev.tick as f64;
                if at < from || at >= to {
                    continue;
                }
                let onset = matches!(ev.kind, EventKind::Trigger | EventKind::NoteOn);
                if drums && onset && DrumVoice::of_target(ev.target).is_some_and(|v| self.bar.voices & v.bit() == 0) {
                    continue;
                }
                let offset = (((at - t0) * samples_per_tick).round().max(0.0) as usize).min(frames - 1) as u32;
                let fired = LiveEvent { vel: scale_vel(ev.vel, gate.vel), ..*ev };
                self.sampler.handle_event(&fired, offset);
            }
        }
    }
}

fn scale_vel(vel: u8, scale: f32) -> u8 {
    if vel == 0 {
        return 0;
    }
    ((vel as f32 * scale).round() as u8).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::DrumLayout;
    use crate::kit::{grid_slices, mudpie_bank, share, DrumKit};
    use crate::sampler::SampleData;

    const SR: u32 = 48_000;

    fn tone(freq: f32, secs: f32, decay: f32) -> SampleData {
        let n = (SR as f32 * secs) as usize;
        let s: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                (std::f32::consts::TAU * freq * t).sin() * (-t / decay).exp() * 0.5
            })
            .collect();
        SampleData::from_channels(&s, &s, SR, 36)
    }

    fn noise(secs: f32, decay: f32, seed: u32) -> SampleData {
        let n = (SR as f32 * secs) as usize;
        let mut x = seed.max(1);
        let s: Vec<f32> = (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let t = i as f32 / SR as f32;
                (x as f32 / u32::MAX as f32 - 0.5) * (-t / decay).exp()
            })
            .collect();
        SampleData::from_channels(&s, &s, SR, 60)
    }

    fn drum_player(seed: u32) -> Player {
        let kit = DrumKit {
            kick: Some(share(tone(55.0, 0.4, 0.12))),
            snare: Some(share(noise(0.3, 0.08, 7))),
            hats_closed: vec![share(noise(0.08, 0.02, 3))],
            hats_open: vec![share(noise(0.4, 0.15, 5))],
            break_loop: Some(share(grid_slices(noise(1.4, 10.0, 9), 16))),
            ..Default::default()
        };
        let (layout, bank) = kit.build();
        let mut sampler = Sampler::new(SR, 32);
        sampler.set_bank(basedrop::Shared::new(&crate::gc::gc_handle(), bank));
        Player::new(PatternLane::drums(layout, seed, seed), sampler)
    }

    fn bass_player(seed: u32) -> Player {
        let mut sampler = Sampler::new(SR, 16);
        sampler.set_bank(basedrop::Shared::new(&crate::gc::gc_handle(), mudpie_bank(share(tone(43.65, 8.0, 100.0)))));
        Player::new(PatternLane::bass(seed, seed), sampler)
    }

    /// Hold `key` from song start and render `bars` bars at `bpm` in blocks of `block`.
    fn render(p: &mut Player, bars: u32, bpm: f64, block: usize) -> Vec<f32> {
        p.note_on(41, 1.0, 0.0);
        let total = (bars as f64 * 4.0 * 60.0 / bpm * SR as f64) as usize;
        let mut out = vec![0.0; total];
        let mut buf = vec![StereoSample::silence(); block];
        let mut pos = 0;
        while pos < total {
            let n = block.min(total - pos);
            let beat = pos as f64 * bpm / 60.0 / SR as f64;
            p.process(&mut buf[..n], &Transport { playing: true, beat, bpm });
            for (o, s) in out[pos..pos + n].iter_mut().zip(&buf[..n]) {
                *o = s.left;
            }
            pos += n;
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn output_does_not_depend_on_the_block_size() {
        for make in [drum_player as fn(u32) -> Player, bass_player] {
            let a = render(&mut make(4), 4, 172.0, 64);
            let b = render(&mut make(4), 4, 172.0, 517);
            assert!(rms(&a) > 1e-3, "plays");
            let diff = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
            assert!(diff < 1e-4, "max diff {diff}");
        }
    }

    #[test]
    fn same_seed_renders_the_same_audio() {
        let a = render(&mut drum_player(9), 2, 170.0, 256);
        let b = render(&mut drum_player(9), 2, 170.0, 256);
        assert_eq!(a, b);
        let c = render(&mut drum_player(10), 2, 170.0, 256);
        assert_ne!(a, c);
    }

    #[test]
    fn silent_without_a_note() {
        let mut p = drum_player(1);
        let mut buf = vec![StereoSample::silence(); 4800];
        for i in 0..20 {
            p.process(&mut buf, &Transport { playing: true, beat: i as f64 * 0.25, bpm: 150.0 });
            assert!(buf.iter().all(|s| s.left == 0.0 && s.right == 0.0));
        }
    }

    #[test]
    fn note_off_closes_on_the_next_step() {
        let bpm = 120.0;
        let frames = (2.0 * 4.0 * 60.0 / bpm * SR as f64) as usize;
        let run = |release: bool| {
            let mut p = drum_player(2);
            p.note_on(36, 1.0, 0.0);
            if release {
                // Released a little after the second 16th: the gate closes at step 2.
                p.note_off(36, TICKS_PER_STEP as f64 * 1.3);
            }
            let mut out = vec![0.0f32; frames];
            let mut buf = vec![StereoSample::silence(); 64];
            let mut pos = 0;
            while pos < frames {
                let n = 64.min(frames - pos);
                let beat = pos as f64 * bpm / 60.0 / SR as f64;
                p.process(&mut buf[..n], &Transport { playing: true, beat, bpm });
                for (o, s) in out[pos..pos + n].iter_mut().zip(&buf[..n]) {
                    *o = s.left;
                }
                pos += n;
            }
            (out, p.is_gated())
        };
        let (held, gated) = run(false);
        let (released, still) = run(true);
        assert!(gated && !still);
        // One second in, everything that started before the close has rung out.
        let tail = SR as usize..frames;
        assert!(rms(&held[tail.clone()]) > 1e-3);
        assert!(rms(&released[tail]) < 1e-4);
        // The first 16th played the same in both.
        let head = 0..(SR as f64 * 60.0 / bpm / 4.0) as usize;
        assert_eq!(held[head.clone()], released[head]);
    }

    #[test]
    fn retrigger_restarts_the_pattern_at_the_note() {
        let mut a = drum_player(3);
        a.phase = Phase::Retrigger;
        let mut b = drum_player(3);
        // Continuous playback from song start, against retrigger from bar 5.
        let ref_audio = render(&mut b, 1, 160.0, 128);
        let bpm = 160.0;
        let start = 5.0 * 4.0;
        a.note_on(41, 1.0, start * TICKS_PER_BEAT);
        let frames = ref_audio.len();
        let mut out = vec![0.0; frames];
        let mut buf = vec![StereoSample::silence(); 128];
        let mut pos = 0;
        while pos < frames {
            let n = 128.min(frames - pos);
            let beat = start + pos as f64 * bpm / 60.0 / SR as f64;
            a.process(&mut buf[..n], &Transport { playing: true, beat, bpm });
            for (o, s) in out[pos..pos + n].iter_mut().zip(&buf[..n]) {
                *o = s.left;
            }
            pos += n;
        }
        let diff = out.iter().zip(&ref_audio).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
        assert!(diff < 1e-4, "bar 0 again: {diff}");
    }

    #[test]
    fn processing_does_not_allocate() {
        let mut p = drum_player(5);
        p.note_on(36, 0.8, 0.0);
        let mut buf = vec![StereoSample::silence(); 256];
        p.process(&mut buf, &Transport { playing: true, beat: 0.0, bpm: 174.0 });
        let (_, allocs) = crate::test_support::count_allocs(|| {
            for i in 1..400 {
                let beat = i as f64 * 256.0 * 174.0 / 60.0 / SR as f64;
                p.process(&mut buf, &Transport { playing: true, beat, bpm: 174.0 });
            }
        });
        assert_eq!(allocs, 0);
        let _ = DrumLayout::standard();
    }
}
