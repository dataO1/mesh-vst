//! mesh vst: the CLAP and VST3 plugin around `mesh-vst-core`.
//!
//! One instance plays one lane, drums or bass (design D1). A held note gates
//! the pattern and gives the root (D7); the host's song position places the
//! bars (D6). With the transport stopped, a held note runs its own clock from
//! the note-on, so the plugin can be played live.

use std::sync::Arc;

use mesh_vst_core::event::{EventKind, LiveEvent};
use mesh_vst_core::gc::gc_handle;
use mesh_vst_core::kit::{mudpie_bank, share};
use mesh_vst_core::macros::{self, Centers, Knobs};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::player::{Phase, Player, Transport, TICKS_PER_BEAT};
use mesh_vst_core::sampler::{LaneSamplerParams, Sampler};
use mesh_vst_core::types::StereoSample;
use nice_plug::prelude::*;

pub mod builtin;
pub mod params;

use params::{MeshParams, Mode, PhaseMode, BEND_RANGES};

/// Tempo when the host gives none.
const DEFAULT_BPM: f64 = 120.0;
/// Sampler voices per mode.
const DRUM_VOICES: usize = 48;
const BASS_VOICES: usize = 16;
// MUSICAL: output trims of the built-in sounds, measured to peak below 0 dBFS
// (the library samples get loudness-normalized at load, M3).
const BUILTIN_DRUM_GAIN: f32 = 0.4;
const BUILTIN_BASS_GAIN: f32 = 0.8;

/// Everything the knobs and settings resolve to; a change re-applies them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    knobs: Knobs,
    sound_seed: u32,
    pattern_seed: u32,
    form: usize,
    rate: i8,
    minor: bool,
    mode: u8,
    phase: Phase,
}

/// One mode's player and the sampler params the knobs start from.
struct Lane {
    player: Player,
    base: LaneSamplerParams,
}

pub struct MeshVst {
    params: Arc<MeshParams>,
    sample_rate: f32,
    drums: Option<Lane>,
    bass: Option<Lane>,
    buf: Vec<StereoSample>,
    applied: Option<(Mode, Settings)>,
    /// Mod wheel 0..1: pushes Intensity up (D24).
    mod_wheel: f32,
    /// Position of the free clock in beats, while the transport is stopped.
    free_beat: f64,
    free_running: bool,
}

impl Default for MeshVst {
    fn default() -> Self {
        Self {
            params: Arc::new(MeshParams::default()),
            sample_rate: 48_000.0,
            drums: None,
            bass: None,
            buf: Vec::new(),
            applied: None,
            mod_wheel: 0.0,
            free_beat: 0.0,
            free_running: false,
        }
    }
}

impl MeshVst {
    fn settings(&self) -> Settings {
        let p = &self.params;
        let intensity = p.intensity.value();
        let (minor, mode) = p.scale.value().family_mode();
        Settings {
            knobs: Knobs {
                density: p.density.value(),
                intensity: intensity + self.mod_wheel * (1.0 - intensity),
                variation: p.variation.value(),
                swing: p.swing.value(),
                shape: p.shape.value(),
                morph: p.morph.value(),
                spread: p.spread.value(),
                slide: p.slide.value(),
            },
            sound_seed: p.sound_seed.value() as u32,
            pattern_seed: p.pattern_seed.value() as u32,
            form: p.form.value().max(0) as usize,
            rate: p.rate.value().exponent(),
            minor,
            mode,
            phase: match p.phase.value() {
                PhaseMode::Continuous => Phase::Continuous,
                PhaseMode::Retrigger => Phase::Retrigger,
            },
        }
    }

    fn lane_mut(&mut self, mode: Mode) -> Option<&mut Lane> {
        match mode {
            Mode::Drums => self.drums.as_mut(),
            Mode::Bass => self.bass.as_mut(),
        }
    }

    /// Re-apply the knobs and settings when they changed. RT-safe.
    fn sync(&mut self, mode: Mode) {
        let s = self.settings();
        if self.applied == Some((mode, s)) {
            return;
        }
        let Some(lane) = self.lane_mut(mode) else { return };
        let role = lane.player.lane().role();
        let resolved = Centers::of(role, s.pattern_seed, s.sound_seed).resolve(&s.knobs);
        let mut sp = lane.base;
        {
            let pl = lane.player.lane_mut();
            pl.pattern_seed = s.pattern_seed;
            pl.sound_seed = s.sound_seed;
            pl.rate = s.rate;
            pl.mode = s.mode;
            pl.select_form(s.form);
            macros::apply(role, &resolved, pl, &mut sp);
        }
        lane.player.minor = s.minor;
        lane.player.phase = s.phase;
        lane.player.sampler_mut().set_params(sp);
        self.applied = Some((mode, s));
    }

    /// Handle one note or controller event at song tick `tick`.
    fn handle(&mut self, mode: Mode, ev: NoteEvent<()>, tick: f64, playing: bool) {
        let bend_range = BEND_RANGES[self.params.bend_range.value().clamp(0, 6) as usize];
        if matches!(ev, NoteEvent::NoteOn { .. }) && !playing && !self.lane_mut(mode).is_some_and(|l| l.player.is_gated()) {
            // A note with the transport stopped starts the free clock.
            self.free_beat = 0.0;
            self.free_running = true;
        }
        if let NoteEvent::MidiCC { cc: 1, value, .. } = ev {
            self.mod_wheel = value;
            return;
        }
        let tick = if playing { tick } else { self.free_beat * TICKS_PER_BEAT };
        let Some(lane) = self.lane_mut(mode) else { return };
        match ev {
            NoteEvent::NoteOn { key, velocity, .. } => {
                if let Some(k) = key.number() {
                    lane.player.note_on(k, velocity, tick);
                }
            }
            NoteEvent::NoteOff { key, .. } => {
                if let Some(k) = key.number() {
                    lane.player.note_off(k, tick);
                }
            }
            NoteEvent::Choke { .. } => lane.player.sampler_mut().reset_voices(),
            NoteEvent::MidiPitchBend { value, .. } => {
                let cents = ((value - 0.5) * 2.0 * bend_range as f32 * 100.0).round() as i16;
                lane.player.sampler_mut().handle_event(&LiveEvent { kind: EventKind::Bend, cents, ..Default::default() }, 0);
            }
            NoteEvent::MidiCC { cc, value, .. } => {
                let vel = (value * 127.0).round() as u8;
                lane.player.sampler_mut().handle_event(&LiveEvent { kind: EventKind::Cc, target: cc, vel, ..Default::default() }, 0);
            }
            _ => {}
        }
    }

    /// Render frames `from..to` of the block into `out`.
    fn render(&mut self, mode: Mode, out: &mut [&mut [f32]], from: usize, to: usize, transport: Transport, gain: &Smoother<f32>) {
        let n = to - from;
        let lane = match mode {
            Mode::Drums => self.drums.as_mut(),
            Mode::Bass => self.bass.as_mut(),
        };
        let buf = &mut self.buf[..n];
        match lane {
            Some(l) => l.player.process(buf, &transport),
            None => buf.fill(StereoSample::silence()),
        }
        for (i, s) in self.buf[..n].iter().enumerate() {
            let g = gain.next();
            out[0][from + i] = s.left * g;
            out[1][from + i] = s.right * g;
        }
    }
}

impl Plugin for MeshVst {
    const NAME: &'static str = "mesh vst";
    const VENDOR: &'static str = "dataO1";
    const URL: &'static str = "https://github.com/dataO1/mesh-vst";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn activate(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl ActivateContext<Self>) -> bool {
        self.sample_rate = config.sample_rate;
        let sr = config.sample_rate.round() as u32;
        self.buf = vec![StereoSample::silence(); config.max_buffer_size as usize];

        let (layout, bank) = builtin::drum_kit(sr).build();
        let mut sampler = Sampler::new(sr, DRUM_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), bank));
        self.drums = Some(Lane { player: Player::new(PatternLane::drums(layout, 1, 1), sampler), base: LaneSamplerParams { gain: BUILTIN_DRUM_GAIN, ..macros::drum_params() } });

        let mut sampler = Sampler::new(sr, BASS_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), mudpie_bank(share(builtin::mudpie(sr)))));
        self.bass = Some(Lane { player: Player::new(PatternLane::bass(1, 1), sampler), base: LaneSamplerParams { gain: BUILTIN_BASS_GAIN, ..macros::mudpie_params() } });
        self.applied = None;
        true
    }

    fn reset(&mut self) {
        for lane in [self.drums.as_mut(), self.bass.as_mut()].into_iter().flatten() {
            lane.player.sampler_mut().reset_voices();
        }
        self.free_running = false;
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let mode = self.params.mode.value();
        self.sync(mode);
        let (bpm, host_playing, block_beat) = {
            let t = context.transport();
            (t.tempo.unwrap_or(DEFAULT_BPM), t.playing, t.pos_beats().unwrap_or(0.0))
        };
        let frames = buffer.samples().min(self.buf.len());
        let beats_per_frame = bpm / 60.0 / self.sample_rate as f64;
        let params = self.params.clone();
        let out = buffer.as_slice();

        let mut cursor = 0usize;
        let mut pending = context.next_event();
        loop {
            // Events at the cursor first, then audio up to the next event.
            while let Some(ev) = pending.filter(|e| e.timing() as usize <= cursor) {
                pending = context.next_event();
                let tick = (block_beat + cursor as f64 * beats_per_frame) * TICKS_PER_BEAT;
                self.handle(mode, ev, tick, host_playing);
                self.sync(mode);
            }
            if cursor >= frames {
                break;
            }
            let next = pending.map_or(frames, |e| (e.timing() as usize).clamp(cursor + 1, frames));
            let gated = self.lane_mut(mode).is_some_and(|l| l.player.is_gated());
            let transport = if host_playing {
                self.free_running = false;
                Transport { playing: true, beat: block_beat + cursor as f64 * beats_per_frame, bpm }
            } else {
                Transport { playing: self.free_running, beat: self.free_beat, bpm }
            };
            self.render(mode, out, cursor, next, transport, &params.gain.smoothed);
            if !host_playing && self.free_running {
                self.free_beat += (next - cursor) as f64 * beats_per_frame;
                self.free_running = gated;
            }
            cursor = next;
        }
        while let Some(ev) = pending {
            let tick = (block_beat + frames as f64 * beats_per_frame) * TICKS_PER_BEAT;
            self.handle(mode, ev, tick, host_playing);
            pending = context.next_event();
        }
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for MeshVst {
    const CLAP_ID: &'static str = "io.github.dataO1.mesh-vst";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Seeded sampler for breakbeat drums and mudpie bass");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Drum, ClapFeature::Stereo];
}

impl Vst3Plugin for MeshVst {
    const VST3_CLASS_ID: [u8; 16] = *b"meshvst-dataO1-1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler, Vst3SubCategory::Drum];
}

nice_export_clap!(MeshVst);
nice_export_vst3!(MeshVst);
