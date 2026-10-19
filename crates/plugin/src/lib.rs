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
use mesh_vst_core::macros::{self, Knobs, LaneSettings};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::player::{Phase, Player, Transport, TICKS_PER_BEAT};
use mesh_vst_core::sampler::{LaneSamplerParams, Sampler};
use mesh_vst_core::types::StereoSample;
use nice_plug::prelude::*;

pub mod builtin;
pub mod editor;
pub mod params;
pub mod ui;
pub mod worker;

use params::{MeshParams, Mode, PhaseMode, BEND_RANGES};
use worker::{Garbage, Ready, Request, Worker};

/// Tempo when the host gives none.
const DEFAULT_BPM: f64 = 120.0;
/// Sampler voices per mode.
pub(crate) const DRUM_VOICES: usize = 48;
pub(crate) const BASS_VOICES: usize = 16;
// MUSICAL: gains of the built-in sounds until their first calibration lands
// (a few hundred ms after loading), measured near the target.
/// Environment variable that makes the plugin hold a note from activation
/// (screenshots of the playing editor, without MIDI input).
const DEMO_ENV: &str = "MESH_VST_DEMO";
const START_DRUM_GAIN: f32 = 0.4;
const START_BASS_GAIN: f32 = 0.8;

/// Everything the knobs and settings resolve to; a change re-applies them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    lane: LaneSettings,
    minor: bool,
    phase: Phase,
}

/// What the loudness calibration depends on: the samples, seeds, form and
/// rate, not the knobs (Mesh calibrated at the default controls too).
fn calibration_key(s: &LaneSettings) -> (u32, u32, usize, i8, u8, Option<u8>) {
    (s.sound_seed, s.pattern_seed, s.form, s.rate, s.mode, s.style)
}

/// One mode's player and the sampler params the knobs start from.
struct Lane {
    player: Player,
    base: LaneSamplerParams,
    /// Settings and kit generation the playing kit was calibrated for, and
    /// the ones asked for.
    loaded: Option<(LaneSettings, u32)>,
    requested: Option<(LaneSettings, u32)>,
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
    worker: Option<Worker>,
    ui: Arc<ui::UiShared>,
    editor_state: Arc<nice_plug_iced::IcedEditorState>,
    /// Last bar render published to the editor.
    published: (u64, u64),
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
            worker: None,
            ui: Arc::new(ui::UiShared::default()),
            editor_state: nice_plug_iced::IcedEditorState::from_size(editor::WINDOW, 1.0),
            published: (u64::MAX, u64::MAX),
        }
    }
}

impl MeshVst {
    fn settings(&self) -> Settings {
        let p = &self.params;
        let intensity = p.intensity.value();
        let (minor, mode) = p.scale.value().family_mode();
        Settings {
            lane: LaneSettings {
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
            mode,
            style: p.style.value().index(),
            },
            minor,
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

    /// Swap in kits the worker finished. The replaced lane goes back to it.
    fn take_kits(&mut self) {
        let Some(w) = self.worker.as_mut() else { return };
        while let Ok(ready) = w.ready.pop() {
            match ready {
                Ready::Drums { mut lane, bank, loop_bpm, gain, settings, generation } => {
                    if let Some(l) = self.drums.as_mut() {
                        l.player.swap_lane(&mut lane);
                        l.player.sampler_mut().set_bank(bank);
                        l.player.loop_bpm = loop_bpm;
                        l.base.gain = gain;
                        l.loaded = Some((settings, generation));
                    }
                    let _ = w.garbage.push(Garbage::Lane(lane));
                }
                Ready::Bass { bank, gain, settings, generation } => {
                    if let Some(l) = self.bass.as_mut() {
                        l.player.sampler_mut().set_bank(bank);
                        l.base.gain = gain;
                        l.loaded = Some((settings, generation));
                    }
                }
            }
            self.applied = None;
        }
    }

    /// Ask the worker for the kit and its loudness trim when the samples,
    /// seeds or form are not the ones playing or already asked for.
    fn ask_kit(&mut self, mode: Mode) {
        let want = self.settings().lane;
        let key = calibration_key(&want);
        let Some(w) = self.worker.as_mut() else { return };
        let lane = match mode {
            Mode::Drums => self.drums.as_mut(),
            Mode::Bass => self.bass.as_mut(),
        };
        let Some(l) = lane else { return };
        let generation = self.ui.kit_generation.load(std::sync::atomic::Ordering::Acquire);
        let key = (key, generation);
        let have = |s: &Option<(LaneSettings, u32)>| s.as_ref().map(|(s, g)| (calibration_key(s), *g));
        if have(&l.loaded) == Some(key) || have(&l.requested) == Some(key) {
            return;
        }
        let generation = self.ui.kit_generation.load(std::sync::atomic::Ordering::Acquire);
        let req = match mode {
            Mode::Drums => Request::Drums(want, generation),
            Mode::Bass => Request::Bass(want, generation),
        };
        if w.requests.push(req).is_ok() {
            l.requested = Some((want, generation));
        }
    }

    /// Hand the editor what it shows. Atomics only: RT-safe.
    fn publish(&mut self, mode: Mode, playing: bool, bpm: f64) {
        use std::sync::atomic::Ordering::Relaxed;
        self.ui.bpm.store((bpm as f32).to_bits(), Relaxed);
        let Some(lane) = (match mode {
            Mode::Drums => self.drums.as_ref(),
            Mode::Bass => self.bass.as_ref(),
        }) else {
            return;
        };
        let p = &lane.player;
        self.ui.root.store(p.gate_key().unwrap_or(255), Relaxed);
        self.ui.gated.store(p.is_gated(), Relaxed);
        let step = p.position().filter(|_| playing || self.free_running);
        self.ui.step.store(step.map_or(u32::MAX, |(_, s)| s), Relaxed);
        if let Some((bar, rendered)) = p.rendered() {
            let key = (bar, p.render_count());
            if key != self.published {
                self.published = key;
                self.ui.bar.store(bar as u32, Relaxed);
                self.ui.publish_bar(p.lane().role(), rendered);
            }
        }
        if self.editor_state.is_open() {
            self.ui.notifier.notify();
        }
    }

    /// Re-apply the knobs and settings when they changed. RT-safe.
    fn sync(&mut self, mode: Mode) {
        let s = self.settings();
        if self.applied == Some((mode, s)) {
            return;
        }
        let Some(lane) = self.lane_mut(mode) else { return };
        let base = lane.base;
        let sp = macros::configure(lane.player.lane_mut(), base, &s.lane);
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

    type Editor = nice_plug_iced::IcedEditor;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        editor::create(self.editor_state.clone(), editor::EditorData { params: self.params.clone(), shared: self.ui.clone() })
    }

    fn activate(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl ActivateContext<Self>) -> bool {
        self.sample_rate = config.sample_rate;
        let sr = config.sample_rate.round() as u32;
        self.buf = vec![StereoSample::silence(); config.max_buffer_size as usize];

        let (layout, bank) = builtin::drum_kit(sr).build();
        let mut sampler = Sampler::new(sr, DRUM_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), bank));
        self.drums = Some(Lane { player: Player::new(PatternLane::drums(layout, 1, 1), sampler), base: LaneSamplerParams { gain: START_DRUM_GAIN, ..macros::drum_params() }, loaded: None, requested: None });

        let mut sampler = Sampler::new(sr, BASS_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), mudpie_bank(share(builtin::mudpie(sr)))));
        self.bass = Some(Lane { player: Player::new(PatternLane::bass(1, 1), sampler), base: LaneSamplerParams { gain: START_BASS_GAIN, ..macros::mudpie_params() }, loaded: None, requested: None });
        self.applied = None;
        // The built-in sounds play until the library kit is loaded.
        self.worker = Some(Worker::spawn(sr, self.params.kit.clone()));
        // Demo for screenshots and checks: hold a note from the start.
        if std::env::var_os(DEMO_ENV).is_some() {
            for lane in [self.drums.as_mut(), self.bass.as_mut()].into_iter().flatten() {
                lane.player.note_on(41, 1.0, 0.0);
            }
            self.free_beat = 0.0;
            self.free_running = true;
        }
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
        self.take_kits();
        self.ask_kit(mode);
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
        self.publish(mode, host_playing, bpm);
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
