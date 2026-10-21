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
use crate::generator::bass::home_root;
use crate::generator::layout::{DrumVoice, BREAK_BASE};
use crate::key::MusicalKey;
use crate::pattern::{FormFollow, PatternLane, RenderedBar, Scratch};
use crate::role::Role;
use crate::sampler::Sampler;
use crate::splice::{self, Occupancy};
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

/// What a new held note does to the sounding bass (D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewRoot {
    /// The pattern's next notes glide over from the held voice.
    #[default]
    Glide,
    /// The sounding notes stop; the pattern restarts its line at the new root.
    Retrigger,
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
    pub new_root: NewRoot,
    /// Scale family of the root: minor or major.
    pub minor: bool,
    gate: Option<Gate>,
    /// Song tick the last block ended at.
    until: Option<f64>,
    bar: RenderedBar,
    scratch: Scratch,
    /// What `bar` was rendered for: pattern bar, lane version, root key.
    cached: Option<(u64, u64, u8, u64)>,
    version: u64,
    /// Tempo the break and top loops were recorded at: they play faster or
    /// slower with the host, the pitch following (design D29).
    pub loop_bpm: Option<f64>,
    /// Bar renders so far (the editor redraws its view on a new one).
    renders: u64,
    /// Pattern bar and 16th step at the end of the last block.
    position: Option<(u64, u32)>,
    /// Events fired in the last block with their frame offsets (mod outputs).
    fired: Box<[(u32, LiveEvent); MAX_FIRED]>,
    fired_len: usize,
    /// Splice: the amount (0.5 = off) and what the host plays, by song bar.
    splice: f32,
    host: [Occupancy; HOST_BARS],
    /// What this lane plays by song bar, for followers: (song bar, version,
    /// root) it was worked out for, and the result.
    published: [(u64, u64, u8, Occupancy); HOST_BARS],
    side: RenderedBar,
    side_scratch: Scratch,
}

/// Song bars of a host a follower keeps (the playing one and the next ones).
pub const HOST_BARS: usize = 4;

/// Events a block keeps for the mod outputs.
pub const MAX_FIRED: usize = 512;

impl Player {
    pub fn new(lane: PatternLane, mut sampler: Sampler) -> Self {
        let sample_rate = sampler.sample_rate() as f64;
        // Drums: each tail group on its own bus ([`crate::tail::VoiceTails`]);
        // without a bus buffer they all play on the main output.
        if lane.role() == Role::Drums {
            sampler.set_bus_map(crate::tail::bus_map());
        }
        Self {
            lane,
            sampler,
            sample_rate,
            phase: Phase::default(),
            new_root: NewRoot::default(),
            minor: true,
            gate: None,
            until: None,
            bar: RenderedBar::default(),
            scratch: Scratch::default(),
            cached: None,
            version: 0,
            loop_bpm: None,
            renders: 0,
            position: None,
            fired: Box::new([(0, LiveEvent::default()); MAX_FIRED]),
            fired_len: 0,
            splice: 0.5,
            host: [Occupancy::default(); HOST_BARS],
            published: [(u64::MAX, 0, 0, Occupancy::default()); HOST_BARS],
            side: RenderedBar::default(),
            side_scratch: Scratch::default(),
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

    /// Swap the lane with `lane` (a new kit layout). `lane` then holds the
    /// old one, for the caller to drop off the audio thread. RT-safe.
    pub fn swap_lane(&mut self, lane: &mut PatternLane) {
        self.version = self.version.wrapping_add(1);
        self.cached = None;
        std::mem::swap(&mut self.lane, lane);
    }

    pub fn sampler(&self) -> &Sampler {
        &self.sampler
    }

    pub fn sampler_mut(&mut self) -> &mut Sampler {
        &mut self.sampler
    }

    /// The pattern bar playing, if any (the last block's).
    pub fn rendered(&self) -> Option<(u64, &RenderedBar)> {
        self.cached.map(|(bar, _, _, _)| (bar, &self.bar))
    }

    /// A note starts at song tick `tick` with velocity 0..1. The newest held
    /// note wins (D10).
    pub fn note_on(&mut self, key: u8, vel: f32, tick: f64) {
        let origin = match (self.phase, self.gate) {
            (Phase::Continuous, _) => 0.0,
            // Legato into a new note keeps the running pattern.
            (Phase::Retrigger, Some(g)) if g.close_at.is_infinite() => g.origin,
            // Exactly at the note, so the first step always fires.
            (Phase::Retrigger, _) => tick,
        };
        if !self.is_gated() {
            // A fresh note plays the same sounds each time.
            self.sampler.reset_round_robin();
            // The block cursor can sit a rounding error past the note; a step
            // exactly at the note still fires.
            self.until = self.until.map(|u| u.min(tick));
        }
        let changed = self.gate.is_some_and(|g| g.close_at.is_infinite() && g.key % 12 != key % 12);
        if changed && self.new_root == NewRoot::Retrigger && self.lane.role() != Role::Drums {
            self.sampler.handle_event(&LiveEvent { kind: EventKind::Choke, ..Default::default() }, 0);
        }
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

    /// The held note's key.
    pub fn gate_key(&self) -> Option<u8> {
        self.gate.filter(|g| g.close_at.is_infinite()).map(|g| g.key)
    }

    /// The events the last block fired, with their frame offsets.
    pub fn fired(&self) -> &[(u32, LiveEvent)] {
        &self.fired[..self.fired_len]
    }

    /// Bar renders so far.
    pub fn render_count(&self) -> u64 {
        self.renders
    }

    /// Pattern bar and 16th step the last block ended in, while a note plays.
    pub fn position(&self) -> Option<(u64, u32)> {
        self.position
    }

    /// Whether a note holds the gate open.
    pub fn is_gated(&self) -> bool {
        self.gate.is_some_and(|g| g.close_at.is_infinite())
    }

    /// Splice: follow a host at `amount` (0 stacked .. 0.5 off .. 1 gaps),
    /// with what it plays by song bar (unknown bars: `song_bar` u64::MAX).
    /// A bar renders again when the focus it gets changes. RT-safe.
    pub fn set_splice(&mut self, amount: f32, host: &[Occupancy]) {
        self.splice = amount;
        for (dst, src) in self.host.iter_mut().zip(host.iter().chain(std::iter::repeat(&Occupancy::default()))) {
            *dst = *src;
        }
    }

    /// The focus of pattern bar `bar` (bar 0 at song tick `origin`): neutral
    /// when Splice is off or a song bar under it is unknown.
    fn focus_for(&self, origin: f64, bar: u64) -> [f32; crate::STEPS_PER_BAR] {
        let neutral = [1.0; crate::STEPS_PER_BAR];
        if (self.splice - 0.5).abs() < 1e-3 {
            return neutral;
        }
        let mut occ = [0.0f32; crate::STEPS_PER_BAR];
        for (s, o) in occ.iter_mut().enumerate() {
            // The middle of the step, in song ticks.
            let t = origin + bar as f64 * TICKS_PER_BAR as f64 + (s as f64 + 0.5) * TICKS_PER_STEP as f64;
            let song_bar = (t / TICKS_PER_BAR as f64).floor().max(0.0) as u64;
            let step = ((t.rem_euclid(TICKS_PER_BAR as f64)) / TICKS_PER_STEP as f64) as usize;
            let Some(h) = self.host.iter().find(|h| h.song_bar == song_bar) else { return neutral };
            *o = h.steps[step.min(crate::STEPS_PER_BAR - 1)] as f32 / 255.0;
        }
        splice::focus(self.splice, &occ)
    }

    /// The host's data under song tick `t`: its occupancy and the step.
    fn host_at(&self, t: f64) -> Option<(&Occupancy, usize)> {
        let song_bar = (t / TICKS_PER_BAR as f64).floor().max(0.0) as u64;
        let step = ((t.rem_euclid(TICKS_PER_BAR as f64)) / TICKS_PER_STEP as f64) as usize;
        self.host.iter().find(|h| h.song_bar == song_bar).map(|h| (h, step.min(crate::STEPS_PER_BAR - 1)))
    }

    /// A pitched host's line under pattern bar `bar` (bar 0 at `origin`),
    /// its keys moved into this lane's key frame (`shift`: the held note
    /// over the home root), and the relation the Splice amount sets (D46).
    fn host_line_for(&self, origin: f64, bar: u64, shift: i32) -> splice::HostLine {
        let mut line = splice::HostLine { relation: 2.0 * self.splice.clamp(0.0, 1.0) - 1.0, ..Default::default() };
        for s in 0..crate::STEPS_PER_BAR {
            let t = origin + bar as f64 * TICKS_PER_BAR as f64 + (s as f64 + 0.5) * TICKS_PER_STEP as f64;
            let Some((h, step)) = self.host_at(t) else { return splice::HostLine::default() };
            if h.flags & splice::PITCHED == 0 {
                return splice::HostLine::default();
            }
            if h.keys[step] != 0 {
                line.keys[s] = (h.keys[step] as i32 - shift).clamp(1, 127) as u8;
                line.lens[s] = h.lens[step];
            }
            if h.onsets & (1 << step) != 0 {
                line.onsets |= 1 << s;
            }
        }
        // Between its notes the host's line is where it last was: carry the
        // last note on (from the bar's end into its start: a looping line).
        splice::carry(&mut line);
        line.on = true;
        line
    }

    /// The host's form position at the start of pattern bar `bar` (D46).
    fn follow_for(&self, origin: f64, bar: u64) -> Option<FormFollow> {
        let t = origin + bar as f64 * TICKS_PER_BAR as f64;
        let song_bar = (t / TICKS_PER_BAR as f64).floor().max(0.0) as u64;
        let h = self.host.iter().find(|h| h.song_bar == song_bar && h.flags & splice::FORMED != 0)?;
        let into = h.form_offset as f64 + (t - song_bar as f64 * TICKS_PER_BAR as f64);
        let host_bar = h.form_bar as u64 + (into / TICKS_PER_BAR as f64 + 1e-6).floor() as u64;
        let relation = ((2.0 * self.splice.clamp(0.0, 1.0) - 1.0) * 100.0).round() as i8;
        Some(FormFollow { spec: h.form_spec(), bar: host_bar, relation })
    }

    /// Set what Splice gives pattern bar `bar` for the next render: the
    /// focus, the host's line and form position. Returns a stamp of them.
    fn prepare(&mut self, origin: f64, bar: u64, shift: i32) -> u64 {
        let focus = self.focus_for(origin, bar);
        let line = self.host_line_for(origin, bar, shift);
        let follow = self.follow_for(origin, bar);
        let mut stamp = focus.iter().fold(0u64, |h, f| h.rotate_left(5) ^ f.to_bits() as u64);
        if line.on {
            stamp = line.keys.iter().chain(&line.lens).fold(stamp.rotate_left(7) ^ line.relation.to_bits() as u64, |h, v| h.rotate_left(3) ^ *v as u64);
        }
        if let Some(f) = follow {
            let seeds = f.spec.seeds.phrases.iter().chain([&f.spec.seeds.fill, &f.spec.seeds.second]).fold(0u64, |h, s| h.rotate_left(9) ^ *s as u64);
            let bands = (f.spec.variation as u64) | (f.spec.shape as u64) << 8 | (f.spec.drums as u64) << 16;
            stamp = stamp.rotate_left(11) ^ f.bar ^ seeds.rotate_left(23) ^ (bands << 40) ^ ((f.relation as u8 as u64) << 48);
        }
        self.lane.focus = focus;
        self.lane.host_line = line;
        self.lane.set_follow(follow);
        stamp
    }

    /// What this lane plays in song bar `song_bar` (for followers). Silent
    /// while no note is held. Worked out once per bar and lane version.
    pub fn occupancy_for(&mut self, song_bar: u64) -> Occupancy {
        let Some(gate) = self.gate.filter(|g| g.close_at.is_infinite()) else {
            return Occupancy { song_bar, ..Occupancy::default() };
        };
        let key = MusicalKey::new(gate.key % 12, self.minor);
        let want = (song_bar, self.version, key.root);
        if let Some(p) = self.published.iter().find(|p| (p.0, p.1, p.2) == want) {
            return p.3;
        }
        let bar_ticks = TICKS_PER_BAR as f64;
        let start = song_bar as f64 * bar_ticks;
        let mut occ = [0.0f32; crate::STEPS_PER_BAR];
        let pitched = self.lane.role() != Role::Drums;
        let shift = if pitched { gate.key as i32 - home_root(key) } else { 0 };
        let (mut keys, mut lens, mut onsets) = ([0u8; crate::STEPS_PER_BAR], [0u8; crate::STEPS_PER_BAR], 0u16);
        // The pattern bars under this song bar (two when the origin is off the bar line).
        let first = ((start - gate.origin) / bar_ticks).floor();
        for pb in [first, first + 1.0] {
            if pb < 0.0 {
                continue;
            }
            let pb = pb as u64;
            self.prepare(gate.origin, pb, shift);
            self.lane.render(pb, key, &mut self.side, &mut self.side_scratch);
            let own = splice::occupancy(self.lane.role(), &self.side.events, self.side.voices);
            let (own_keys, own_lens, own_onsets) = splice::line(&self.side.events);
            let bar_start = gate.origin + pb as f64 * bar_ticks;
            for (s, v) in own.iter().enumerate() {
                let t = bar_start + s as f64 * TICKS_PER_STEP as f64 - start;
                if (0.0..bar_ticks).contains(&t) {
                    let i = (t / TICKS_PER_STEP as f64).round() as usize;
                    if i < crate::STEPS_PER_BAR {
                        occ[i] = occ[i].max(*v);
                        if pitched && own_keys[s] != 0 {
                            keys[i] = (own_keys[s] as i32 + shift).clamp(1, 127) as u8;
                            lens[i] = own_lens[s];
                        }
                        if own_onsets & (1 << s) != 0 {
                            onsets |= 1 << i;
                        }
                    }
                }
            }
        }
        let mut out = splice::pack(song_bar, &occ);
        if pitched {
            out.keys = keys;
            out.lens = lens;
            out.onsets = onsets;
            out.flags |= splice::PITCHED;
        }
        // The form position: the pattern bar under the song bar's start.
        if let Some(spec) = self.lane.form_spec().filter(|_| first >= 0.0) {
            out.form_bar = first as u32;
            out.form_offset = (start - (gate.origin + first * bar_ticks)).round().clamp(0.0, u16::MAX as f64) as u16;
            out.set_form(&spec);
        }

        // Keep it in the oldest slot.
        let slot = self.published.iter().enumerate().min_by_key(|(_, p)| if p.0 == u64::MAX { 0 } else { p.0.wrapping_add(1) }).map_or(0, |(i, _)| i);
        self.published[slot] = (want.0, want.1, want.2, out);
        out
    }

    /// Render one block into `out` (overwritten).
    pub fn process(&mut self, out: &mut [StereoSample], t: &Transport) {
        self.process_split(out, None, t);
    }

    /// [`Self::process`] with the sampler's bus voices in `bus` (see
    /// [`Sampler::process_split`]).
    pub fn process_split(&mut self, out: &mut [StereoSample], bus: Option<&mut [StereoSample]>, t: &Transport) {
        self.fired_len = 0;
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
            self.position = self.gate.map(|g| {
                let local = (t1 - g.origin).max(0.0);
                let bar = (local / TICKS_PER_BAR as f64) as u64;
                (bar, ((local % TICKS_PER_BAR as f64) / TICKS_PER_STEP as f64) as u32)
            });
            if self.gate.is_some_and(|g| g.close_at <= t1) {
                self.gate = None;
            }
        } else {
            self.until = None;
        }
        self.sampler.set_bpm(bpm);
        self.sampler.process_split(out, bus, samples_per_tick);
    }

    /// Fire the events of song ticks `from..to` (block starts at `t0`).
    fn fire_range(&mut self, from: f64, to: f64, t0: f64, samples_per_tick: f64, frames: usize) {
        let Some(gate) = self.gate else { return };
        let bpm = self.sample_rate * 60.0 / (samples_per_tick * TICKS_PER_BEAT);
        let loop_cents = self.loop_bpm.filter(|b| *b > 0.0).map_or(0, |b| (1200.0 * (bpm / b).log2()).round().clamp(-2400.0, 2400.0) as i16);
        let to = to.min(gate.close_at);
        if to <= from {
            return;
        }
        let bar_ticks = TICKS_PER_BAR as f64;
        let first = ((from - gate.origin) / bar_ticks).floor().max(0.0) as u64;
        let last = ((to - gate.origin) / bar_ticks).floor().max(0.0) as u64;
        let key = MusicalKey::new(gate.key % 12, self.minor);
        // Pitched lanes play in the held note's octave (D7).
        let octave_shift = if self.lane.role() == Role::Drums { 0 } else { gate.key as i32 - home_root(key) };
        for bar in first..=last {
            // Splice: the host's hits, line and form position under this bar.
            let stamp = self.prepare(gate.origin, bar, octave_shift);
            let want = (bar, self.version, key.root, stamp);
            if self.cached != Some(want) {
                self.lane.render(bar, key, &mut self.bar, &mut self.scratch);
                self.cached = Some(want);
                self.renders = self.renders.wrapping_add(1);
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
                let mut fired = LiveEvent { vel: scale_vel(ev.vel, gate.vel), ..*ev };
                if octave_shift != 0 && ev.key != 0 {
                    fired.key = (ev.key as i32 + octave_shift).clamp(1, 127) as u8;
                }
                if drums && ev.target >= BREAK_BASE {
                    fired.cents = fired.cents.saturating_add(loop_cents);
                }
                self.sampler.handle_event(&fired, offset);
                if self.fired_len < MAX_FIRED {
                    self.fired[self.fired_len] = (offset, fired);
                    self.fired_len += 1;
                }
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

    #[test]
    fn splice_moves_the_follower_onto_or_off_the_host() {
        let mut host = drum_player(3);
        host.note_on(29, 1.0, 0.0);
        let occ: Vec<Occupancy> = (0..HOST_BARS as u64).map(|b| host.occupancy_for(b)).collect();
        assert!(occ.iter().all(|o| o.steps.iter().any(|s| *s > 0)), "the host plays");
        let bpm = 175.0;
        let spb = 60.0 / bpm * SR as f64;
        // Share of the follower's onsets on the host's strong hits.
        let on_host = |amount: f32| {
            let mut f = bass_player(5);
            f.lane_mut().set_param("density", 0.8);
            f.set_splice(amount, &occ);
            f.note_on(29, 1.0, 0.0);
            let mut buf = vec![StereoSample::silence(); 64];
            let (mut pos, mut on, mut all) = (0usize, 0.0, 0.0);
            while (pos as f64) < spb * 4.0 * HOST_BARS as f64 {
                f.process(&mut buf, &Transport { playing: true, beat: pos as f64 / spb, bpm });
                for (off, e) in f.fired() {
                    if e.kind != EventKind::NoteOn {
                        continue;
                    }
                    let tick = ((pos as f64 + *off as f64) / spb * TICKS_PER_BEAT).round();
                    let bar = (tick / TICKS_PER_BAR as f64) as usize;
                    let step = ((tick % TICKS_PER_BAR as f64) / TICKS_PER_STEP as f64) as usize;
                    let Some(o) = occ.get(bar) else { continue };
                    on += (o.steps[step.min(15)] > 128) as u8 as f32;
                    all += 1.0;
                }
                pos += 64;
            }
            on / all
        };
        let (stacked, off, gaps) = (on_host(0.0), on_host(0.5), on_host(1.0));
        assert!(stacked > off + 0.05 && gaps < off - 0.05, "stacked {stacked}, off {off}, gaps {gaps}");
        // Mostly, not only: the extremes still play on the other side.
        assert!(stacked < 0.98 && gaps > 0.02, "stacked {stacked}, gaps {gaps}");
        // Deterministic: the same host gives the same line.
        assert_eq!(on_host(1.0), gaps);
    }

    /// The follower's published bars against a pitched host at `amount`
    /// (D46): the host's and the follower's occupancy per song bar.
    fn against_host(amount: f32, bars: u64) -> Vec<(Occupancy, Occupancy)> {
        let mut host = bass_player(3);
        host.lane_mut().set_param("density", 0.7);
        host.lane_mut().set_param("spread", 0.7);
        // Long and short notes both.
        host.lane_mut().set_param("length", 0.8);
        host.lane_mut().set_param("stabs", 0.5);
        host.lane_mut().set_form_spec(crate::macros::FormSettings::seeded(2).spec(false, 0.5, 0.5, 0.5));
        host.note_on(29, 1.0, 0.0);
        let mut f = bass_player(5);
        f.lane_mut().set_param("density", 0.8);
        f.lane_mut().set_param("spread", 0.7);
        f.note_on(29, 1.0, 0.0);
        (0..bars)
            .map(|b| {
                let occ: Vec<Occupancy> = (0..HOST_BARS as u64).map(|k| host.occupancy_for(b + k)).collect();
                f.set_splice(amount, &occ);
                (occ[0], f.occupancy_for(b))
            })
            .collect()
    }

    /// Motion of the follower's notes against the host, over consecutive
    /// follower onsets with a host note under both: (contrary, similar or
    /// parallel, parallel perfect, perfect intervals at the onsets, pairs).
    fn motions(pairs: &[(Occupancy, Occupancy)]) -> (f32, f32, f32, f32, f32) {
        let perfect = |a: i32, b: i32| matches!((a - b).rem_euclid(12), 0 | 7);
        let (mut contrary, mut same, mut par, mut perf, mut n, mut notes) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut prev: Option<(i32, i32)> = None;
        for (h, f) in pairs {
            // The host's line carried between its notes, as the follower hears it.
            let mut line = splice::HostLine { keys: h.keys, lens: h.lens, ..Default::default() };
            splice::carry(&mut line);
            for s in 0..crate::STEPS_PER_BAR {
                if f.onsets & (1 << s) == 0 || f.keys[s] == 0 {
                    continue;
                }
                let (fk, hk) = (f.keys[s] as i32, line.keys[s] as i32);
                if hk == 0 {
                    prev = None;
                    continue;
                }
                notes += 1.0;
                perf += perfect(fk, hk) as u8 as f32;
                if let Some((pf, ph)) = prev {
                    let (df, dh) = (fk - pf, hk - ph);
                    if df != 0 && dh != 0 {
                        n += 1.0;
                        if df.signum() != dh.signum() {
                            contrary += 1.0;
                        } else {
                            same += 1.0;
                            if perfect(fk, hk) && perfect(pf, ph) {
                                par += 1.0;
                            }
                        }
                    }
                }
                prev = Some((fk, hk));
            }
        }
        (contrary / n.max(1.0), same / n.max(1.0), par / n.max(1.0), perf / notes.max(1.0), n)
    }

    #[test]
    fn splice_layers_complements_or_counters_a_pitched_host() {
        let bars = 64;
        let (layer, mid, counter) = (against_host(0.0, bars), against_host(0.5, bars), against_host(1.0, bars));
        assert!(layer.iter().all(|(h, _)| h.flags & splice::PITCHED != 0 && h.onsets != 0), "the host shares its line");
        let (l, m, c) = (motions(&layer), motions(&mid), motions(&counter));
        assert!(l.4 > 20.0 && m.4 > 20.0 && c.4 > 20.0, "enough moving pairs: {l:?} {m:?} {c:?}");
        // Contrary motion rises towards the counter end, similar motion falls.
        assert!(c.0 > m.0 && m.0 > l.0 - 0.05 && c.0 > 0.55, "contrary {l:?} {m:?} {c:?}");
        assert!(l.1 > c.1, "similar or parallel {:.2} vs {:.2}", l.1, c.1);
        // Unisons, octaves and fifths on the onsets: most when layered.
        assert!(l.3 > m.3 + 0.05 && l.3 > c.3 + 0.03, "perfect intervals {:.2} {:.2} {:.2}", l.3, m.3, c.3);
        // No parallel octaves or fifths at the counter end.
        assert!(c.2 < 0.03 && c.2 < l.2, "parallel perfect {:.3} vs layered {:.3}", c.2, l.2);
        // Deterministic.
        assert_eq!(against_host(1.0, 4), counter[..4].to_vec());
    }

    #[test]
    fn splice_blocks_change_with_the_host() {
        let bpm = 175.0;
        let spb = 60.0 / bpm * SR as f64;
        let letters = |follow: Option<f32>| {
            let mut host = bass_player(3);
            host.lane_mut().set_form_spec(crate::macros::FormSettings::seeded(3).spec(false, 0.5, 0.5, 0.5));
            host.note_on(29, 1.0, 0.0);
            let mut f = drum_player(9);
            f.lane_mut().set_form_spec(crate::macros::FormSettings::seeded(8).spec(true, 0.5, 0.5, 0.5));
            f.note_on(29, 1.0, 0.0);
            let mut buf = vec![StereoSample::silence(); 256];
            let (mut pos, mut same, mut bars) = (0usize, 0, 0);
            let mut last = u64::MAX;
            while (pos as f64) < spb * 4.0 * 48.0 {
                let bar = (pos as f64 / spb / 4.0) as u64;
                if let Some(amount) = follow {
                    let occ: Vec<Occupancy> = (0..HOST_BARS as u64).map(|k| host.occupancy_for(bar + k)).collect();
                    f.set_splice(amount, &occ);
                }
                let t = Transport { playing: true, beat: pos as f64 / spb, bpm };
                host.process(&mut buf, &t);
                f.process(&mut buf, &t);
                if let (Some((hb, h)), Some((fb, fr))) = (host.rendered(), f.rendered()) {
                    if hb == fb && fb != last {
                        last = fb;
                        bars += 1;
                        same += (h.letter == fr.letter) as u32;
                    }
                }
                pos += 256;
            }
            (same, bars)
        };
        // Stacked, the follower's form is nearly the host's (D48); in the
        // middle and alone its own.
        let (same, bars) = letters(Some(0.0));
        assert!(bars > 40 && same as f32 > 0.75 * bars as f32, "stacked, the follower plays the host's letters: {same} of {bars}");
        let (middle, _) = letters(Some(0.5));
        let (own, _) = letters(None);
        assert!(middle < same && own < same, "its own form differs: {middle} and {own} of {bars}");

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
    fn a_new_root_retriggers_or_glides() {
        for (mode, cut) in [(NewRoot::Retrigger, true), (NewRoot::Glide, false)] {
            let mut p = bass_player(6);
            p.new_root = mode;
            p.note_on(41, 1.0, 0.0);
            let bpm = 150.0;
            let mut buf = vec![StereoSample::silence(); 256];
            let mut pos = 0usize;
            // Play until a note sounds.
            while p.sampler().voices_active() == 0 && pos < 48_000 * 4 {
                let beat = pos as f64 * bpm / 60.0 / SR as f64;
                p.process(&mut buf, &Transport { playing: true, beat, bpm });
                pos += 256;
            }
            let before = p.sampler().voices_active();
            assert!(before > 0);
            p.note_on(43, 1.0, pos as f64 * bpm / 60.0 / SR as f64 * TICKS_PER_BEAT);
            // One short block, before the next 16th: only the choke acts.
            let beat = pos as f64 * bpm / 60.0 / SR as f64;
            p.process(&mut buf[..16], &Transport { playing: true, beat, bpm });
            let mut quiet = vec![StereoSample::silence(); 960];
            p.process(&mut quiet, &Transport { playing: false, beat, bpm });
            let level = quiet[900..].iter().map(|s| s.left.abs()).fold(0.0, f32::max);
            assert_eq!(level < 1e-4, cut, "{mode:?}: {level}");
        }
    }

    /// Press `key` at each beat of `presses`, release 6 beats later; the fired
    /// events of each press as (tick from the press, target, key, velocity).
    fn presses(p: &mut Player, key: u8, presses: &[f64]) -> Vec<Vec<(i64, u8, u8, u8)>> {
        let bpm = 175.0;
        let spb = 60.0 / bpm * SR as f64;
        let block = 64usize;
        let mut logs = vec![Vec::new(); presses.len()];
        let mut buf = vec![StereoSample::silence(); block];
        let mut pos = 0usize;
        let mut cur = None;
        let end = presses.last().map_or(0.0, |b| b + 8.0);
        while (pos as f64) < spb * end {
            let beat = pos as f64 / spb;
            let next = (pos + block) as f64 / spb;
            for (i, &b) in presses.iter().enumerate() {
                if (beat..next).contains(&b) {
                    p.note_on(key, 1.0, beat * TICKS_PER_BEAT);
                    cur = Some((i, beat * TICKS_PER_BEAT));
                }
                if (beat..next).contains(&(b + 6.0)) {
                    p.note_off(key, beat * TICKS_PER_BEAT);
                }
            }
            p.process(&mut buf, &Transport { playing: true, beat, bpm });
            if let Some((i, origin)) = cur {
                for (off, e) in p.fired() {
                    let at = ((pos as f64 + *off as f64) / spb * TICKS_PER_BEAT - origin).round() as i64;
                    logs[i].push((at, e.target, e.key, e.vel));
                }
            }
            pos += block;
        }
        logs
    }

    #[test]
    fn every_retrigger_plays_the_same_from_its_first_step() {
        let mut p = drum_player(5);
        p.phase = Phase::Retrigger;
        // Off the 16th grid both ways, so no rounding can drop the downbeat.
        let logs = presses(&mut p, 36, &[3.3, 13.7, 21.05, 30.5]);
        assert!(logs[0].first().is_some_and(|e| e.0 == 0), "the first step fires: {:?}", &logs[0][..3]);
        for l in &logs[1..] {
            assert_eq!(l, &logs[0]);
        }
    }

    #[test]
    fn the_held_octave_moves_the_bass() {
        let low = presses(&mut bass_player(6), 29, &[0.0]);
        let high = presses(&mut bass_player(6), 41, &[0.0]);
        let keys = |l: &Vec<(i64, u8, u8, u8)>| l.iter().filter(|e| e.2 != 0).map(|e| e.2 as i32).collect::<Vec<_>>();
        let (lo, hi) = (keys(&low[0]), keys(&high[0]));
        assert!(!lo.is_empty() && lo.len() == hi.len());
        assert!(lo.iter().zip(&hi).all(|(a, b)| b - a == 12), "{lo:?} {hi:?}");
        // The held F1 plays the home register: its root is F1.
        assert!(lo.contains(&29), "{lo:?}");
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

