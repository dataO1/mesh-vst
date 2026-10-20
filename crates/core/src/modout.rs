//! Modulation outputs (design D32, D34): up to 16 DC control channels the
//! plugin writes to aux outputs, so the DAW's own followers can map them onto
//! any effect.
//!
//! Every channel is a level 0..1, derived sample-accurately from the events
//! the player fired and from the transport. The default sources follow D34:
//!
//! | Ch | Bass | Drums |
//! |---|---|---|
//! | 1 | velocity (held) | velocity (held) |
//! | 2 | stab | accent |
//! | 3 | amp envelope | kick |
//! | 4 | filter envelope | snare |
//! | 5 | LFO | hats |
//! | 6 | scrub position (held) | cymbals |
//! | 7 | glide | break and perc |
//! | 8 | note pitch (held) | fill (filler letters) |
//! | 9..16 | beat ramp, bar ramp, phrase position, form letter, density, permute, mod wheel, pressure | the same |

use crate::event::{flags, EventKind, LiveEvent};
use crate::generator::layout::DrumVoice;
use crate::role::Role;

/// Channels.
pub const CHANNELS: usize = 16;
/// Channel names, for the host's port list.
pub const NAMES_BASS: [&str; CHANNELS] = [
    "Mod 1 velocity",
    "Mod 2 stab",
    "Mod 3 amp env",
    "Mod 4 filter env",
    "Mod 5 LFO",
    "Mod 6 scrub",
    "Mod 7 glide",
    "Mod 8 pitch",
    "Mod 9 beat ramp",
    "Mod 10 bar ramp",
    "Mod 11 phrase",
    "Mod 12 form letter",
    "Mod 13 density",
    "Mod 14 permute",
    "Mod 15 mod wheel",
    "Mod 16 pressure",
];
/// Bars of the phrase channel's ramp.
const PHRASE_BARS: f64 = 8.0;

// MUSICAL: envelope times of the derived channels, first draft.
const ACCENT_SECS: f32 = 0.15;
const AMP_DECAY_SECS: f32 = 0.6;
const AMP_SUSTAIN: f32 = 0.7;
const AMP_RELEASE_SECS: f32 = 0.12;
const FILTER_SECS: f32 = 0.4;
const HIT_SECS: f32 = 0.12;
const GLIDE_SECS: f32 = 0.2;
/// Lowest and highest key of the pitch channel's range.
const PITCH_RANGE: (f32, f32) = (24.0, 60.0);

/// What changes per block apart from the events.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockInfo {
    pub playing: bool,
    /// Song position at the block start and its rate, in beats.
    pub beat: f64,
    pub beats_per_frame: f64,
    /// Form letter of the playing bar (0..=3).
    pub letter: u8,
    pub density: f32,
    pub permute: f32,
    pub mod_wheel: f32,
    pub pressure: f32,
    /// The bass LFO's rate in Hz.
    pub lfo_hz: f32,
}

/// A channel's source (D34: every channel can be reassigned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Velocity,
    Accent,
    AmpEnv,
    FilterEnv,
    Lfo,
    Scrub,
    Glide,
    Pitch,
    Kick,
    Snare,
    Hats,
    Cymbals,
    BreakPerc,
    Fill,
    BeatRamp,
    BarRamp,
    Phrase,
    FormLetter,
    Density,
    Permute,
    ModWheel,
    Pressure,
    Off,
}

/// Sources.
pub const SOURCES: usize = 23;

impl Source {
    pub const ALL: [Source; SOURCES] = [
        Source::Velocity,
        Source::Accent,
        Source::AmpEnv,
        Source::FilterEnv,
        Source::Lfo,
        Source::Scrub,
        Source::Glide,
        Source::Pitch,
        Source::Kick,
        Source::Snare,
        Source::Hats,
        Source::Cymbals,
        Source::BreakPerc,
        Source::Fill,
        Source::BeatRamp,
        Source::BarRamp,
        Source::Phrase,
        Source::FormLetter,
        Source::Density,
        Source::Permute,
        Source::ModWheel,
        Source::Pressure,
        Source::Off,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Source::Velocity => "velocity",
            Source::Accent => "accent / stab",
            Source::AmpEnv => "amp env",
            Source::FilterEnv => "filter env",
            Source::Lfo => "LFO",
            Source::Scrub => "scrub",
            Source::Glide => "glide",
            Source::Pitch => "pitch",
            Source::Kick => "kick",
            Source::Snare => "snare",
            Source::Hats => "hats",
            Source::Cymbals => "cymbals",
            Source::BreakPerc => "break / perc",
            Source::Fill => "fill",
            Source::BeatRamp => "beat ramp",
            Source::BarRamp => "bar ramp",
            Source::Phrase => "phrase",
            Source::FormLetter => "form letter",
            Source::Density => "density",
            Source::Permute => "permute",
            Source::ModWheel => "mod wheel",
            Source::Pressure => "pressure",
            Source::Off => "off",
        }
    }
}

/// D34's default sources per mode.
pub const DEFAULT_BASS: [Source; CHANNELS] = [
    Source::Velocity,
    Source::Accent,
    Source::AmpEnv,
    Source::FilterEnv,
    Source::Lfo,
    Source::Scrub,
    Source::Glide,
    Source::Pitch,
    Source::BeatRamp,
    Source::BarRamp,
    Source::Phrase,
    Source::FormLetter,
    Source::Density,
    Source::Permute,
    Source::ModWheel,
    Source::Pressure,
];
pub const DEFAULT_DRUMS: [Source; CHANNELS] = [
    Source::Velocity,
    Source::Accent,
    Source::Kick,
    Source::Snare,
    Source::Hats,
    Source::Cymbals,
    Source::BreakPerc,
    Source::Fill,
    Source::BeatRamp,
    Source::BarRamp,
    Source::Phrase,
    Source::FormLetter,
    Source::Density,
    Source::Permute,
    Source::ModWheel,
    Source::Pressure,
];

/// The channels' running state.
pub struct ModOut {
    sample_rate: f32,
    /// Source per channel, per mode.
    pub map_bass: [Source; CHANNELS],
    pub map_drums: [Source; CHANNELS],
    velocity: f32,
    accent: f32,
    amp: f32,
    amp_target: f32,
    filter: f32,
    scrub: f32,
    glide: f32,
    pitch: f32,
    hits: [f32; 5],
    lfo_phase: f32,
    /// Notes held (NoteOn without its NoteOff yet), for the amp envelope.
    held: u32,
}

impl ModOut {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            map_bass: DEFAULT_BASS,
            map_drums: DEFAULT_DRUMS,
            velocity: 0.0,
            accent: 0.0,
            amp: 0.0,
            amp_target: 0.0,
            filter: 0.0,
            scrub: 0.0,
            glide: 0.0,
            pitch: 0.0,
            hits: [0.0; 5],
            lfo_phase: 0.0,
            held: 0,
        }
    }

    fn coeff(&self, secs: f32) -> f32 {
        (-1.0 / (secs * self.sample_rate).max(1.0)).exp()
    }

    fn fire(&mut self, role: Role, e: &LiveEvent) {
        let onset = matches!(e.kind, EventKind::Trigger | EventKind::NoteOn) && e.vel > 0;
        if e.kind == EventKind::NoteOff {
            self.held = self.held.saturating_sub(1);
            if self.held == 0 {
                self.amp_target = 0.0;
            }
            return;
        }
        if !onset {
            return;
        }
        let vel = e.vel as f32 / 127.0;
        self.velocity = vel;
        if e.has(flags::ACCENT) || e.has(flags::STAB) {
            self.accent = 1.0;
        }
        match role {
            Role::Drums => {
                let i = match DrumVoice::of_target(e.target) {
                    Some(DrumVoice::Kick) => 0,
                    Some(DrumVoice::Snare) => 1,
                    Some(DrumVoice::Hats) => 2,
                    Some(DrumVoice::Ride | DrumVoice::Shaker) => 3,
                    _ => 4,
                };
                self.hits[i] = self.hits[i].max(vel);
            }
            _ => {
                if e.kind == EventKind::NoteOn {
                    self.held += 1;
                }
                self.amp = self.amp.max(vel);
                self.amp_target = if e.has(flags::STAB) || e.gate == 0 { 0.0 } else { AMP_SUSTAIN * vel };
                self.filter = 1.0;
                self.scrub = e.start as f32 / u16::MAX as f32;
                if e.has(flags::TIE) {
                    self.glide = 1.0;
                }
                if e.key > 0 {
                    self.pitch = ((e.key as f32 - PITCH_RANGE.0) / (PITCH_RANGE.1 - PITCH_RANGE.0)).clamp(0.0, 1.0);
                }
            }
        }
    }

    /// Render one block. `events` are the player's fired events with their
    /// frame offsets, sorted. `out` holds up to [`CHANNELS`] channel buffers.
    pub fn render(&mut self, role: Role, events: &[(u32, LiveEvent)], info: &BlockInfo, out: &mut [&mut [f32]]) {
        let frames = out.iter().map(|c| c.len()).min().unwrap_or(0);
        let (accent_k, amp_k, release_k, filter_k, hit_k, glide_k) = (
            self.coeff(ACCENT_SECS),
            self.coeff(AMP_DECAY_SECS),
            self.coeff(AMP_RELEASE_SECS),
            self.coeff(FILTER_SECS),
            self.coeff(HIT_SECS),
            self.coeff(GLIDE_SECS),
        );
        let lfo_step = info.lfo_hz / self.sample_rate;
        let fill = if info.letter >= 2 { 1.0 } else { 0.0 };
        let letter = info.letter as f32 / 3.0;
        let mut next = 0;
        for i in 0..frames {
            while next < events.len() && events[next].0 as usize <= i {
                let e = events[next].1;
                self.fire(role, &e);
                next += 1;
            }
            let beat = info.beat + i as f64 * info.beats_per_frame;
            let (beat_ramp, bar_ramp, phrase) = if info.playing {
                (beat.fract() as f32, (beat / 4.0).fract() as f32, ((beat / 4.0) % PHRASE_BARS / PHRASE_BARS) as f32)
            } else {
                (0.0, 0.0, 0.0)
            };
            let all: [f32; SOURCES] = [
                self.velocity,
                self.accent,
                self.amp,
                self.filter,
                0.5 + 0.5 * (std::f32::consts::TAU * self.lfo_phase).sin(),
                self.scrub,
                self.glide,
                self.pitch,
                self.hits[0],
                self.hits[1],
                self.hits[2],
                self.hits[3],
                self.hits[4],
                fill,
                beat_ramp,
                bar_ramp,
                phrase,
                letter,
                info.density,
                info.permute,
                info.mod_wheel,
                info.pressure,
                0.0,
            ];
            let map = match role {
                Role::Drums => &self.map_drums,
                _ => &self.map_bass,
            };
            for (c, s) in out.iter_mut().zip(map) {
                c[i] = all[*s as usize];
            }
            self.accent *= accent_k;
            self.filter *= filter_k;
            self.glide *= glide_k;
            for h in &mut self.hits {
                *h *= hit_k;
            }
            let k = if self.amp_target > 0.0 { amp_k } else { release_k };
            self.amp = self.amp_target + (self.amp - self.amp_target) * k;
            self.lfo_phase = (self.lfo_phase + lfo_step).fract();
        }
        // Events past the block end still count.
        for (_, e) in &events[next..] {
            self.fire(role, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: EventKind, target: u8, vel: u8, flags: u8) -> LiveEvent {
        LiveEvent { kind, target, vel, flags, key: 41, gate: 48, ..Default::default() }
    }

    #[test]
    fn hits_light_their_channel_and_decay() {
        let mut m = ModOut::new(48_000.0);
        let mut bufs = vec![vec![0.0f32; 4800]; CHANNELS];
        let events = [(100u32, ev(EventKind::Trigger, 0, 127, flags::ACCENT)), (2400, ev(EventKind::Trigger, 1, 64, 0))];
        let info = BlockInfo { playing: true, beat: 0.0, beats_per_frame: 172.0 / 60.0 / 48_000.0, density: 0.4, ..Default::default() };
        {
            let mut out: Vec<&mut [f32]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            m.render(Role::Drums, &events, &info, &mut out);
        }
        assert_eq!(bufs[2][99], 0.0, "kick before its hit");
        assert!((bufs[2][100] - 1.0).abs() < 1e-6, "kick at its hit");
        // 120 ms time constant: about 44 % after 98 ms.
        assert!(bufs[2][4799] < 0.5, "kick decays");
        assert!((bufs[3][2400] - 64.0 / 127.0).abs() < 1e-3, "snare at its velocity");
        assert!((bufs[0][3000] - 64.0 / 127.0).abs() < 1e-6, "velocity holds the last hit");
        assert!(bufs[1][150] > 0.9, "accent");
        assert!(bufs[8][4799] > bufs[8][0], "beat ramp rises");
        assert!(bufs[12].iter().all(|v| (*v - 0.4).abs() < 1e-6), "density");
    }

    #[test]
    fn bass_envelopes_follow_notes() {
        let mut m = ModOut::new(48_000.0);
        let mut bufs = vec![vec![0.0f32; 19_200]; CHANNELS];
        let mut on = ev(EventKind::NoteOn, 0, 100, flags::TIE);
        on.start = u16::MAX / 2;
        let off = LiveEvent { kind: EventKind::NoteOff, ..on };
        let info = BlockInfo { lfo_hz: 3.0, ..Default::default() };
        {
            let mut out: Vec<&mut [f32]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            m.render(Role::Bass, &[(0, on), (4800, off)], &info, &mut out);
        }
        assert!(bufs[2][10] > 0.7, "amp opens");
        assert!(bufs[2][4700] > 0.5, "amp sustains while held");
        assert!(bufs[2][19_199] < 0.1, "amp releases");
        assert!(bufs[3][10] > bufs[3][4000], "filter env decays");
        assert!((bufs[5][100] - 0.5).abs() < 0.01, "scrub position");
        assert!(bufs[6][10] > 0.9 && bufs[6][19_000] < 0.2, "glide (200 ms time constant)");
        assert!(bufs[4].iter().any(|v| *v > 0.9) && bufs[4].iter().any(|v| *v < 0.1), "LFO swings");
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;

    #[test]
    fn a_reassigned_channel_plays_its_new_source() {
        let mut m = ModOut::new(48_000.0);
        m.map_drums[0] = Source::Kick;
        m.map_drums[1] = Source::Off;
        let mut bufs = vec![vec![0.0f32; 480]; CHANNELS];
        let kick = LiveEvent { kind: EventKind::Trigger, target: 0, vel: 127, ..Default::default() };
        {
            let mut out: Vec<&mut [f32]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
            m.render(Role::Drums, &[(10, kick)], &BlockInfo::default(), &mut out);
        }
        assert!((bufs[0][10] - 1.0).abs() < 1e-6, "channel 1 follows the kick now");
        assert!(bufs[1].iter().all(|v| *v == 0.0), "channel 2 is off");
        assert!((bufs[2][10] - 1.0).abs() < 1e-6, "channel 3 keeps its default (kick)");
    }
}
