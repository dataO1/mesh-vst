//! mesh vst: the CLAP and VST3 plugin around `mesh-vst-core`.
//!
//! One instance plays one lane, drums or bass (design D1). A held note gates
//! the pattern and gives the root (D7); the host's song position places the
//! bars (D6). With the transport stopped, a held note runs its own clock from
//! the note-on, so the plugin can be played live.

use std::sync::Arc;

use mesh_vst_core::event::{flags, EventKind, LiveEvent};
use mesh_vst_core::gc::gc_handle;
use mesh_vst_core::kit::{mudpie_bank, share};
use mesh_vst_core::macros::{self, Centers, Knobs, LaneSettings};
use mesh_vst_core::modout::{self, BlockInfo, ModOut};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::player::{NewRoot, Phase, Player, Transport, TICKS_PER_BEAT};
use mesh_vst_core::sampler::{LaneSamplerParams, Sampler, Unison, UnisonMode};
use mesh_vst_core::types::StereoSample;
use nice_plug::prelude::*;

pub mod builtin;
pub mod editor;
pub mod params;
pub mod ui;
pub mod link;
pub mod visual;
pub mod worker;

use params::{MeshParams, Mode, NewRootMode, PhaseMode, UnisonSpread, BEND_RANGES, SEEDED_BEND_BASS, SEEDED_BEND_DRUMS};
use worker::{Garbage, Ready, Request, Worker};

/// Tempo when the host gives none.
const DEFAULT_BPM: f64 = 175.0;
/// Set by test hosts: `process` waits for each kit load, so renders repeat
/// exactly and need no warmup on the clock.
pub const SYNC_LOAD_ENV: &str = "MESH_VST_SYNC_LOAD";
/// Frames per sidechain level reading (Splice).
const SIDECHAIN_CHUNK: usize = 64;
/// Sampler voices per mode.
pub(crate) const DRUM_VOICES: usize = 48;
pub(crate) const BASS_VOICES: usize = 48;
// MUSICAL: gains of the built-in sounds until their first calibration lands
// (a few hundred ms after loading), measured near the target.
/// Environment variable that makes the plugin hold a note from activation
/// (screenshots of the playing editor, without MIDI input).
const DEMO_ENV: &str = "MESH_VST_DEMO";
/// Names of the mod-out ports: the bass source / the drum source (D34).
const MOD_PORT_NAMES: [&str; modout::CHANNELS] = [
    "Mod 1 velocity",
    "Mod 2 stab / accent",
    "Mod 3 amp env / kick",
    "Mod 4 filter env / snare",
    "Mod 5 LFO / hats",
    "Mod 6 scrub / cymbals",
    "Mod 7 glide / break",
    "Mod 8 pitch / fill",
    "Mod 9 beat ramp",
    "Mod 10 bar ramp",
    "Mod 11 phrase",
    "Mod 12 form letter",
    "Mod 13 density",
    "Mod 14 permute",
    "Mod 15 mod wheel",
    "Mod 16 pressure",
];

/// Editor redraws per second.
const REDRAW_FPS: f32 = 30.0;
const START_DRUM_GAIN: f32 = 0.4;
const START_BASS_GAIN: f32 = 0.8;

/// Everything the knobs and settings resolve to; a change re-applies them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    lane: LaneSettings,
    minor: bool,
    phase: Phase,
    phrase_bars: u32,
    new_root: NewRoot,
    /// Channel or MPE pressure, in 1/64 steps (it drives the stab drive).
    pressure: u8,
}

/// What the loudness calibration depends on: the samples, seeds, form and
/// rate, not the knobs (Mesh calibrated at the default controls too).
fn calibration_key(s: &LaneSettings) -> (u32, u32, usize, i8, u8, Option<u8>, (u8, u32, u8)) {
    let u = (s.unison.voices, s.unison.detune_cents.to_bits(), s.unison.mode as u8);
    (s.sound_seed, s.pattern_seed, s.form, s.rate, s.mode, s.style, u)
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
    /// Mod wheel 0..1: pushes Permute up (D24).
    mod_wheel: f32,
    /// Channel pressure 0..1 (mod out 16).
    pressure: f32,
    /// The knobs' values after the seed's centres (mod out 13, 14).
    resolved: Knobs,
    /// The Heat stage on the lane's output.
    heat: mesh_vst_core::heat::Heat,
    /// The drums' ring modulator (Morph).
    ring: mesh_vst_core::heat::RingMod,
    /// The drums' release tails (Shape).
    tail: mesh_vst_core::tail::Tail,
    /// The monoizer below 80 Hz.
    mono: mesh_vst_core::heat::MonoLows,
    /// Splice (D42): the shared table, the sidechain's hits, this lane's
    /// band levels and its label there.
    link: Option<link::Link>,
    sidechain: link::SidechainOcc,
    meter: link::BandMeter,
    label: [u8; 24],
    /// The knob positions and the seed's centres they play around.
    knobs: Knobs,
    centres: Centers,
    modout: ModOut,
    /// Position of the free clock in beats, while the transport is stopped.
    free_beat: f64,
    free_running: bool,
    worker: Option<Worker>,
    /// The library preview voice and its buffer (D35).
    audition: Option<Sampler>,
    abuf: Vec<StereoSample>,
    ui: Arc<ui::UiShared>,
    editor_state: Arc<nice_plug_iced::IcedEditorState>,
    /// Last bar render published to the editor.
    published: (u64, u64),
    /// The output for the editor's visual, and frames since its last redraw.
    audio_tx: Option<rtrb::Producer<f32>>,
    since_redraw: usize,
    /// Wait for the worker's kits in `process` ([`SYNC_LOAD_ENV`]).
    sync_load: bool,
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
            pressure: 0.0,
            resolved: Knobs::CENTERED,
            heat: mesh_vst_core::heat::Heat::default(),
            ring: mesh_vst_core::heat::RingMod::default(),
            tail: mesh_vst_core::tail::Tail::new(48_000.0),
            mono: mesh_vst_core::heat::MonoLows::default(),
            link: None,
            sidechain: link::SidechainOcc::default(),
            meter: link::BandMeter::new(48_000.0),
            label: [0; 24],
            knobs: Knobs::CENTERED,
            centres: Knobs::CENTERED,
            modout: ModOut::new(48_000.0),
            free_beat: 0.0,
            free_running: false,
            worker: None,
            audition: None,
            abuf: Vec::new(),
            ui: Arc::new(ui::UiShared::default()),
            editor_state: nice_plug_iced::IcedEditorState::from_size(editor::WINDOW, 1.0),
            published: (u64::MAX, u64::MAX),
            audio_tx: None,
            since_redraw: 0,
            sync_load: std::env::var_os(SYNC_LOAD_ENV).is_some(),
        }
    }
}

impl MeshVst {
    fn settings(&self) -> Settings {
        let p = &self.params;
        let permute = p.permute.value();
        let (minor, mode) = p.scale.value().family_mode();
        Settings {
            lane: LaneSettings {
            knobs: Knobs {
                density: p.density.value(),
                permute: permute + self.mod_wheel * (1.0 - permute),
                variation: p.variation.value(),
                swing: p.swing.value(),
                shape: p.shape.value(),
                morph: p.morph.value(),
                spread: p.spread.value(),
                slide: p.slide.value(),
                tone: p.tone.value(),
                heat: p.heat.value(),
            },
            sound_seed: p.sound_seed.value() as u32,
            pattern_seed: p.pattern_seed.value() as u32,
            form: p.form.value().max(0) as usize,
            rate: p.rate.value().exponent(),
            mode,
            style: p.style.value().index(),
            unison: Unison {
                voices: p.unison_voices.value().clamp(1, 8) as u8,
                detune_cents: p.unison_detune.value(),
                width: p.unison_width.value(),
                // (Variation sets it, in `macros::configure`.)
                range: 0.0,
                mode: match p.unison_mode.value() {
                    UnisonSpread::Linear => UnisonMode::Linear,
                    UnisonSpread::Soft => UnisonMode::Soft,
                    UnisonSpread::Hard => UnisonMode::Hard,
                    UnisonSpread::Stacked => UnisonMode::Stacked,
                    UnisonSpread::Random => UnisonMode::Random,
                },
                seed: p.sound_seed.value() as u32,
            },
            },
            minor,
            phase: match p.phase.value() {
                PhaseMode::Continuous => Phase::Continuous,
                PhaseMode::Retrigger => Phase::Retrigger,
            },
            phrase_bars: p.phrase.value().bars(),
            new_root: match p.new_root.value() {
                NewRootMode::Glide => NewRoot::Glide,
                NewRootMode::Retrigger => NewRoot::Retrigger,
            },
            pressure: (self.pressure * 64.0).round() as u8,
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
                Ready::Audition { bank } => {
                    if let Some(a) = self.audition.as_mut() {
                        a.reset_voices();
                        a.set_bank(bank);
                        let ev = LiveEvent { kind: EventKind::Trigger, target: 0, vel: 110, ..Default::default() };
                        a.handle_event(&ev, 0);
                    }
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

    /// Test hosts ([`SYNC_LOAD_ENV`]): block until the asked-for kit landed, so
    /// a render never waits on the clock for the worker. Not RT-safe.
    fn wait_for_kit(&mut self, mode: Mode) {
        let key = |s: &Option<(LaneSettings, u32)>| s.as_ref().map(|(s, g)| (calibration_key(s), *g));
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(60) {
            let Some(l) = self.lane_mut(mode) else { return };
            if l.requested.is_none() || key(&l.requested) == key(&l.loaded) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.take_kits();
        }
    }

    /// Hand the editor what it shows. Atomics only: RT-safe.
    fn publish(&mut self, mode: Mode, playing: bool, bpm: f64, frames: usize) {
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
        // Redraw at about 30 fps (D35).
        self.since_redraw += frames;
        if self.editor_state.is_open() && self.since_redraw as f32 >= self.sample_rate / REDRAW_FPS {
            self.since_redraw = 0;
            self.ui.notifier.notify();
        }
    }

    /// Hand the editor the newest hit the last render fired: the knob
    /// positions its bar and note played with, and its level. RT-safe.
    fn publish_hit(&self, mode: Mode) {
        if !self.editor_state.is_open() {
            return;
        }
        let Some(lane) = (match mode {
            Mode::Drums => self.drums.as_ref(),
            Mode::Bass => self.bass.as_ref(),
        }) else {
            return;
        };
        let p = &lane.player;
        let onset = |e: &LiveEvent| matches!(e.kind, EventKind::NoteOn | EventKind::Trigger) && e.vel > 0;
        let (Some((_, ev)), Some((_, bar))) = (p.fired().iter().rev().find(|(_, e)| onset(e)), p.rendered()) else { return };
        let role = p.lane().role();
        let k = macros::hit_knobs(role, &self.centres, &self.knobs, p.lane().param_names(), &bar.params, ev);
        let sp = p.sampler().params();
        let accent = if ev.has(flags::ACCENT) { sp.accent_gain } else { 1.0 };
        let level = (ev.vel.min(127) as f32 / 127.0).powf(sp.velocity_curve) * accent;
        let a = k.as_array();
        self.ui.publish_hit(std::array::from_fn(|i| a.get(i).copied().unwrap_or(level)));
    }

    /// Splice (D42): share what this lane plays in the next song bars, and
    /// follow the host the editor picked. RT-safe.
    fn splice(&mut self, mode: Mode, beat: f64) {
        use std::sync::atomic::Ordering::Relaxed;
        let bar = (beat * TICKS_PER_BEAT / mesh_vst_core::TICKS_PER_BAR as f64).floor().max(0.0) as u64;
        let host = self.ui.host.load(Relaxed);
        let amount = self.params.splice.value();
        let Some(link) = self.link.as_mut() else { return };
        let lane = match mode {
            Mode::Drums => self.drums.as_mut(),
            Mode::Bass => self.bass.as_mut(),
        };
        let Some(l) = lane else { return };
        let mut data = link::SlotData { follows: host, bands: self.meter.levels(), label: self.label, ..Default::default() };
        for (k, b) in data.bars.iter_mut().enumerate() {
            *b = l.player.occupancy_for(bar + k as u64);
        }
        link.publish(&data);
        let followed = match host {
            0 => None,
            link::SIDECHAIN => Some(self.sidechain.bars(bar)),
            id => link.read(id).map(|d| {
                for (a, v) in self.ui.host_bands.iter().zip(d.bands) {
                    a.store(v, Relaxed);
                }
                d.bars
            }),
        };
        match followed {
            Some(bars) => l.player.set_splice(amount, &bars),
            None => l.player.set_splice(0.5, &[]),
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
        let role = lane.player.lane().role();
        let mut sp = macros::configure(lane.player.lane_mut(), base, &s.lane);
        // Pressure drives the drive (D24): the stab punch, the drum accents.
        let pressure = s.pressure as f32 / 64.0;
        sp.punch = (sp.punch + 0.8 * pressure).min(1.5);
        sp.accent_gain *= 1.0 + 0.5 * pressure;
        let centres = Centers::of(role, s.lane.pattern_seed, s.lane.sound_seed);
        self.resolved = centres.resolve(&s.lane.knobs);
        self.centres = centres;
        self.knobs = s.lane.knobs;
        let Some(lane) = self.lane_mut(mode) else { return };
        lane.player.minor = s.minor;
        lane.player.phase = s.phase;
        lane.player.new_root = s.new_root;
        lane.player.lane_mut().phrase_bars = s.phrase_bars;
        lane.player.sampler_mut().set_params(sp);
        let name = if mode == Mode::Drums { "drums" } else { "bass" };
        self.label = link::label(name, s.lane.sound_seed as i32, self.ui.link_id.load(std::sync::atomic::Ordering::Relaxed));
        self.applied = Some((mode, s));
    }

    /// Pitch bend range in semitones: the setting, or the pattern seed's pick
    /// for the mode (D24).
    fn bend_range(&self, mode: Mode) -> i32 {
        let v = self.params.bend_range.value();
        if v > 0 {
            return BEND_RANGES[(v as usize - 1).min(BEND_RANGES.len() - 1)];
        }
        let seed = self.params.pattern_seed.value() as u64;
        let pick = |t: &[i32]| t[(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 59) as usize % t.len()];
        match mode {
            Mode::Drums => pick(&SEEDED_BEND_DRUMS),
            Mode::Bass => pick(&SEEDED_BEND_BASS),
        }
    }

    /// Handle one note or controller event at song tick `tick`.
    fn handle(&mut self, mode: Mode, ev: NoteEvent<()>, tick: f64, playing: bool) {
        let bend_range = self.bend_range(mode);
        if matches!(ev, NoteEvent::NoteOn { .. }) && !playing && !self.lane_mut(mode).is_some_and(|l| l.player.is_gated()) {
            // A note with the transport stopped starts the free clock.
            self.free_beat = 0.0;
            self.free_running = true;
        }
        if let NoteEvent::MidiChannelPressure { pressure, .. } | NoteEvent::PolyPressure { pressure, .. } = ev {
            self.pressure = pressure;
            return;
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
            // MPE per-note pitch acts as pitch bend (D24).
            NoteEvent::PolyTuning { tuning, .. } => {
                let cents = (tuning * 100.0).round().clamp(-12_000.0, 12_000.0) as i16;
                lane.player.sampler_mut().handle_event(&LiveEvent { kind: EventKind::Bend, cents, ..Default::default() }, 0);
            }
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
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        mode: Mode,
        out: &mut [&mut [f32]],
        aux: &mut AuxiliaryBuffers,
        from: usize,
        to: usize,
        transport: Transport,
        gain: &Smoother<f32>,
    ) {
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
        // Drums: the ring modulator on Morph, and Heat focused by Tone.
        let tone = if mode == Mode::Drums {
            // Release tails above Shape's middle, the shaper in front.
            let at = (transport.beat * 60.0 / transport.bpm.max(1.0) * self.sample_rate as f64).max(0.0) as u64;
            self.tail.process(buf, self.resolved.shape, self.params.tail.value().mode(), at);
            let (hz, mix, shape) = macros::ring(self.params.sound_seed.value() as u32, self.resolved.morph);
            self.ring.process(buf, hz, mix, shape, transport.beat * 60.0 / transport.bpm.max(1.0));
            self.resolved.tone
        } else {
            0.5
        };
        self.heat.process(buf, self.resolved.heat, tone);
        if self.params.mono_lows.value() {
            self.mono.process(buf);
        }
        self.meter.process(buf);
        // Splice: the sidechain's level per 16th of the song.
        if self.ui.host.load(std::sync::atomic::Ordering::Relaxed) == link::SIDECHAIN {
            if let Some(input) = aux.inputs.first_mut() {
                let chans = input.as_slice();
                let ticks_per_frame = transport.bpm / 60.0 / self.sample_rate as f64 * TICKS_PER_BEAT;
                let tick0 = transport.beat * TICKS_PER_BEAT;
                for start in (from..to).step_by(SIDECHAIN_CHUNK) {
                    let end = (start + SIDECHAIN_CHUNK).min(to);
                    let peak = chans.iter().filter_map(|c| c.get(start..end)).flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
                    self.sidechain.feed(tick0 + (start - from) as f64 * ticks_per_frame, peak);
                }
            }
        }
        self.publish_hit(mode);
        // The mod outputs, when the host gave their ports.
        if aux.outputs.len() >= modout::CHANNELS {
            let mut ports = aux.outputs.iter_mut();
            let chans: [Option<&mut [f32]>; modout::CHANNELS] =
                std::array::from_fn(|_| ports.next().and_then(|b| b.as_slice().first_mut().map(|c| &mut c[from..to])));
            if chans.iter().all(Option::is_some) {
                let mut chans = chans.map(|c| c.unwrap_or_default());
                let (role, fired, letter, lfo_hz) = match mode {
                    Mode::Drums => self.drums.as_ref(),
                    Mode::Bass => self.bass.as_ref(),
                }
                .map_or((mesh_vst_core::role::Role::Drums, &[][..], 0, 3.0), |l| {
                    let letter = l.player.rendered().map_or(0, |(_, b)| b.letter);
                    (l.player.lane().role(), l.player.fired(), letter, l.base.modulation.lfo.rate)
                });
                let info = BlockInfo {
                    playing: transport.playing,
                    beat: transport.beat,
                    beats_per_frame: transport.bpm / 60.0 / self.sample_rate as f64,
                    letter,
                    density: self.resolved.density,
                    permute: self.resolved.permute,
                    mod_wheel: self.mod_wheel,
                    pressure: self.pressure,
                    lfo_hz,
                };
                for c in 0..modout::CHANNELS {
                    let at = |m: usize| modout::Source::ALL[self.ui.mod_map[m][c].load(std::sync::atomic::Ordering::Relaxed) as usize % modout::SOURCES];
                    self.modout.map_bass[c] = at(0);
                    self.modout.map_drums[c] = at(1);
                }
                self.modout.render(role, fired, &info, &mut chans);
            }
        }
        if let Some(a) = self.audition.as_mut() {
            let abuf = &mut self.abuf[..n];
            a.process(abuf, 1000.0);
            for (s, x) in self.buf[..n].iter_mut().zip(abuf.iter()) {
                // MUSICAL: previews sit below the lane (samples are at "12 LUFS).
                *s += *x * 0.5;
            }
        }
        let feed = self.editor_state.is_open();
        for (i, s) in self.buf[..n].iter().enumerate() {
            let g = gain.next();
            out[0][from + i] = s.left * g;
            out[1][from + i] = s.right * g;
            if feed {
                if let Some(tx) = self.audio_tx.as_mut() {
                    let _ = tx.push(0.5 * (s.left + s.right) * g);
                }
            }
        }
    }
}

impl Plugin for MeshVst {
    const NAME: &'static str = "mesh vst";
    const VENDOR: &'static str = "dataO1";
    const URL: &'static str = "https://github.com/dataO1/mesh-vst";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Stereo out plus the 16 mono mod-out channels (D32), or stereo only.
    // The first layout is what most hosts take: stereo and the Splice
    // sidechain. The mod outs come in the second (some hosts, Qtractor among
    // them, hand empty buffers for ports they do not route, which nice-plug
    // does not survive).
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: None,
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames { layout: Some("Stereo + sidechain"), main_input: None, main_output: Some("Out"), aux_inputs: &["Sidechain"], aux_outputs: &[] },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: None,
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            aux_output_ports: &[new_nonzero_u32(1); modout::CHANNELS],
            names: PortNames { layout: Some("Stereo + sidechain + mod out"), main_input: None, main_output: Some("Out"), aux_inputs: &["Sidechain"], aux_outputs: &MOD_PORT_NAMES },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: None,
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

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
        self.modout = ModOut::new(config.sample_rate);
        self.heat = mesh_vst_core::heat::Heat::new(config.sample_rate);
        self.ring = mesh_vst_core::heat::RingMod::new(config.sample_rate);
        self.tail = mesh_vst_core::tail::Tail::new(config.sample_rate);
        self.mono = mesh_vst_core::heat::MonoLows::new(config.sample_rate);
        self.meter = link::BandMeter::new(config.sample_rate);
        if let Ok(mut u) = self.params.ui.lock() {
            self.ui.load_mod_map(&u);
            // Splice: a stable id for this instance, saved with the project.
            if u.link_id == 0 || u.link_id == link::SIDECHAIN {
                u.link_id = editor::fresh_id();
            }
            self.ui.link_id.store(u.link_id, std::sync::atomic::Ordering::Relaxed);
            self.ui.host.store(u.host.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        }
        self.link = link::Link::open(self.ui.link_id.load(std::sync::atomic::Ordering::Relaxed)).ok();

        let (layout, bank) = builtin::drum_kit(sr).build();
        let mut sampler = Sampler::new(sr, DRUM_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), bank));
        self.drums = Some(Lane { player: Player::new(PatternLane::drums(layout, 1, 1), sampler), base: LaneSamplerParams { gain: START_DRUM_GAIN, ..macros::drum_params() }, loaded: None, requested: None });

        let mut sampler = Sampler::new(sr, BASS_VOICES);
        sampler.set_bank(basedrop::Shared::new(&gc_handle(), mudpie_bank(share(builtin::mudpie(sr)))));
        self.bass = Some(Lane { player: Player::new(PatternLane::bass(1, 1), sampler), base: LaneSamplerParams { gain: START_BASS_GAIN, ..macros::mudpie_params() }, loaded: None, requested: None });
        self.applied = None;
        // The editor's visual reads the output through a ring (half a second).
        let (tx, rx) = rtrb::RingBuffer::new(sr as usize / 2);
        self.audio_tx = Some(tx);
        if let Ok(mut a) = self.ui.audio.lock() {
            *a = Some(rx);
        }
        self.ui.sample_rate.store(sr, std::sync::atomic::Ordering::Relaxed);
        // The built-in sounds play until the library kit is loaded.
        self.worker = Some(Worker::spawn(sr, self.params.kit.clone(), self.ui.clone()));
        self.audition = Some(Sampler::new(sr, 8));
        self.abuf = vec![StereoSample::silence(); config.max_buffer_size as usize];
        // Demo for screenshots and checks: hold a note from the start.
        if std::env::var_os(DEMO_ENV).is_some() {
            for lane in [self.drums.as_mut(), self.bass.as_mut()].into_iter().flatten() {
                lane.player.note_on(29, 1.0, 0.0);
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

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let mode = self.params.mode.value();
        self.take_kits();
        self.ask_kit(mode);
        if self.sync_load {
            self.wait_for_kit(mode);
        }
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
            self.render(mode, out, aux, cursor, next, transport, &params.gain.smoothed);
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
        self.splice(mode, block_beat);
        self.publish(mode, host_playing, bpm, frames);
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
