//! Headless CLAP host for end-to-end checks of the bundled plugin.
//!
//! ```text
//! hostcheck suite  <bundle.clap> [--library DIR] [--out DIR]   # every scenario, with assertions
//! hostcheck render <bundle.clap> --mode bass --seed 7 --bars 8 --out bass.wav [--stopped] [--release-at 4]
//! ```
//!
//! A render holds `--key` from the first block, sends the transport every
//! block (playing, unless `--stopped`), sets parameters by name, and writes
//! the plugin's stereo output. The suite runs the scenarios in [`suite`]
//! against the built-in sounds and against a library: the folder given with
//! `--library`, else a library of synthesized files it writes itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::audio_ports_config::PluginAudioPortsConfig;
use clack_host::utils::ClapId;
use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent, TransportEvent, TransportFlags};
use clack_host::events::EventFlags;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};

const PLUGIN_ID: &[u8] = b"io.github.dataO1.mesh-vst";
const SR: f64 = 48_000.0;
const BLOCK: usize = 512;
/// Stopped blocks before a render (see `Render::warmup`).
const WARMUP_BLOCKS: usize = 4;
/// The plugin's port layout with the 16 mod outputs.
const MOD_OUT_LAYOUT: u32 = 1;

type Error = Box<dyn std::error::Error>;

struct Host;
struct Shared;

impl<'a> SharedHandler<'a> for Shared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

impl HostHandlers for Host {
    type Shared<'a> = Shared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

/// One render's settings.
#[derive(Debug, Clone)]
struct Render {
    mode: &'static str,
    seed: Option<u32>,
    bars: f64,
    bpm: f64,
    key: u16,
    playing: bool,
    /// The note ends at this bar.
    release_at: Option<f64>,
    /// At this bar the held note changes to this key (note-off, then note-on).
    switch: Option<(f64, u16)>,
    /// Wall-clock seconds of silence first, for background loading.
    warmup: f64,
    params: Vec<(String, f64)>,
}

impl Default for Render {
    fn default() -> Self {
        Self { mode: "drums", seed: None, bars: 4.0, bpm: 172.0, key: 29, playing: true, release_at: None, switch: None, warmup: 0.0, params: Vec::new() }
    }
}

/// A rendered stereo signal, interleaved.
struct Audio {
    samples: Vec<f32>,
    /// The mod-out channels (D32), one buffer each.
    mods: Vec<Vec<f32>>,
}

impl Audio {
    fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    fn secs(&self) -> f64 {
        self.frames() as f64 / SR
    }

    fn peak_db(&self) -> f64 {
        db(self.samples.iter().fold(0f32, |a, b| a.max(b.abs())) as f64)
    }

    /// RMS in dBFS over seconds `from..to`.
    fn rms_db(&self, from: f64, to: f64) -> f64 {
        let a = ((from * SR) as usize * 2).min(self.samples.len());
        let b = ((to * SR) as usize * 2).min(self.samples.len()).max(a);
        let s = &self.samples[a..b];
        let ms = s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len().max(1) as f64;
        db(ms.sqrt())
    }

    /// Integrated loudness (BS.1770).
    fn lufs(&self) -> f64 {
        let st: Vec<mesh_vst_core::types::StereoSample> =
            self.samples.chunks_exact(2).map(|c| mesh_vst_core::types::StereoSample { left: c[0], right: c[1] }).collect();
        mesh_vst_core::loudness::measure_lufs(&st, SR)
    }

    fn finite(&self) -> bool {
        self.samples.iter().all(|v| v.is_finite())
    }

    fn write(&self, path: &Path) -> Result<(), Error> {
        let spec = hound::WavSpec { channels: 2, sample_rate: SR as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(path, spec)?;
        for s in &self.samples {
            w.write_sample(*s)?;
        }
        w.finalize()?;
        Ok(())
    }
}

fn db(v: f64) -> f64 {
    20.0 * v.max(1e-12).log10()
}

fn transport(beat: f64, bpm: f64, playing: bool) -> TransportEvent {
    let mut flags = TransportFlags::HAS_TEMPO | TransportFlags::HAS_BEATS_TIMELINE | TransportFlags::HAS_TIME_SIGNATURE;
    if playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    TransportEvent {
        header: EventHeader::new_core(0, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from_float(beat),
        song_pos_seconds: SecondsTime::from_int(0),
        tempo: bpm,
        tempo_inc: 0.0,
        loop_start_beats: BeatTime::from_int(0),
        loop_end_beats: BeatTime::from_int(0),
        loop_start_seconds: SecondsTime::from_int(0),
        loop_end_seconds: SecondsTime::from_int(0),
        bar_start: BeatTime::from_float((beat / 4.0).floor() * 4.0),
        bar_number: (beat / 4.0).floor() as i32,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
    }
}

/// Instantiate the plugin, play `r`, return its output.
fn render(entry: &PluginEntry, r: &Render) -> Result<Audio, Error> {
    let host_info = HostInfo::new("hostcheck", "mesh vst", "https://github.com/dataO1/mesh-vst", "0.1")?;
    let factory = entry.get_plugin_factory().ok_or("no plugin factory")?;
    let descriptor = factory
        .plugin_descriptors()
        .find(|d| d.id().is_some_and(|id| id.to_bytes() == PLUGIN_ID))
        .ok_or("mesh vst not in the bundle")?;
    let id = descriptor.id().ok_or("no id")?;
    let mut instance = PluginInstance::<Host>::new(|_| Shared, |_| (), entry, id, &host_info)?;

    let mut ids = BTreeMap::new();
    {
        let mut handle = instance.plugin_handle();
        let params = handle.get_extension::<PluginParams>().ok_or("no params extension")?;
        let mut buf = ParamInfoBuffer::new();
        for i in 0..params.count(&mut handle) {
            if let Some(info) = params.get_info(&mut handle, i, &mut buf) {
                ids.insert(String::from_utf8_lossy(info.name).to_string(), info.id);
            }
        }
    }
    let mut set = vec![("Mode".to_string(), if r.mode == "bass" { 1.0 } else { 0.0 })];
    if let Some(seed) = r.seed {
        set.push(("Pattern seed".into(), seed as f64));
        set.push(("Sound seed".into(), seed as f64));
    }
    set.extend(r.params.iter().cloned());
    let mut param_events = Vec::new();
    for (name, value) in &set {
        let id = *ids.get(name).ok_or_else(|| format!("no parameter {name}"))?;
        param_events.push(ParamValueEvent::new(0, id, Pckn::match_all(), *value));
    }

    // The port layout with the mod outs (the second one: hosts take the first,
    // stereo and the sidechain).
    {
        let handle = instance.plugin_handle();
        if let Some(configs) = handle.get_extension::<PluginAudioPortsConfig>() {
            configs.select(&handle, ClapId::new(MOD_OUT_LAYOUT)).map_err(|_| "the plugin refused the mod-out port layout")?;
        }
    }
    // Every output port the plugin declares: the main stereo pair and its mod outputs.
    let channels: Vec<u32> = {
        let mut handle = instance.plugin_handle();
        match handle.get_extension::<PluginAudioPorts>() {
            Some(ports) => {
                let mut buf = AudioPortInfoBuffer::new();
                (0..ports.count(&mut handle, false)).filter_map(|i| ports.get(&mut handle, i, false, &mut buf).map(|p| p.channel_count)).collect()
            }
            None => vec![2],
        }
    };

    // And every input port (the sidechain): silent, as a host with nothing routed there.
    let in_channels: Vec<u32> = {
        let mut handle = instance.plugin_handle();
        match handle.get_extension::<PluginAudioPorts>() {
            Some(ports) => {
                let mut buf = AudioPortInfoBuffer::new();
                (0..ports.count(&mut handle, true)).filter_map(|i| ports.get(&mut handle, i, true, &mut buf).map(|p| p.channel_count)).collect()
            }
            None => Vec::new(),
        }
    };

    let config = PluginAudioConfiguration { sample_rate: SR, min_frames_count: 1, max_frames_count: BLOCK as u32 };
    let processor = instance.activate(|_, _| (), config)?;
    let mut processor = processor.start_processing()?;
    let total_channels: u32 = channels.iter().sum();
    let mut out_ports = AudioPorts::with_capacity(total_channels as usize, channels.len());
    let mut bufs: Vec<Vec<Vec<f32>>> = channels.iter().map(|&c| vec![vec![0.0f32; BLOCK]; c as usize]).collect();
    let mut in_ports = AudioPorts::with_capacity(in_channels.iter().sum::<u32>() as usize, in_channels.len());
    let mut in_bufs: Vec<Vec<Vec<f32>>> = in_channels.iter().map(|&c| vec![vec![0.0f32; BLOCK]; c as usize]).collect();
    let mut out_events = EventBuffer::new();

    let mut block = |events: &EventBuffer, n: usize, bufs: &mut Vec<Vec<Vec<f32>>>| -> Result<(), Error> {
        let input = in_ports.with_input_buffers(in_bufs.iter_mut().map(|port| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(port.iter_mut().map(|c| InputChannel::constant(&mut c[..n]))),
        }));
        let mut output = out_ports.with_output_buffers(bufs.iter_mut().map(|port| AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(port.iter_mut().map(|c| &mut c[..n])),
        }));
        out_events.clear();
        processor.process(&input, &mut output, &events.as_input(), &mut out_events.as_output(), None, None)?;
        Ok(())
    };

    // Warmup: a few stopped blocks so the kit and its calibration land (the
    // plugin waits for them, MESH_VST_SYNC_LOAD).
    let mut first = true;
    for _ in 0..if r.warmup > 0.0 { WARMUP_BLOCKS } else { 0 } {
        let mut events = EventBuffer::new();
        events.push(&transport(0.0, r.bpm, false));
        if std::mem::take(&mut first) {
            for e in &param_events {
                events.push(e);
            }
        }
        block(&events, BLOCK, &mut bufs)?;
    }

    let total = (r.bars * 4.0 * 60.0 / r.bpm * SR) as usize;
    let release = r.release_at.map(|b| (b * 4.0 * 60.0 / r.bpm * SR) as usize);
    let switch = r.switch.map(|(b, k)| ((b * 4.0 * 60.0 / r.bpm * SR) as usize, k));
    let mut samples = Vec::with_capacity(total * 2);
    let mut mods: Vec<Vec<f32>> = bufs.iter().skip(1).flatten().map(|_| Vec::with_capacity(total)).collect();
    let mut pos = 0usize;
    while pos < total {
        let n = BLOCK.min(total - pos);
        let beat = if r.playing { pos as f64 / SR * r.bpm / 60.0 } else { 0.0 };
        let mut events = EventBuffer::new();
        events.push(&transport(beat, r.bpm, r.playing));
        if pos == 0 {
            for e in &param_events {
                events.push(e);
            }
            events.push(&NoteOnEvent::new(0, Pckn::new(0u16, 0u16, r.key, 0u32), 1.0));
        }
        if let Some((at, key)) = switch.filter(|(at, _)| *at >= pos && *at < pos + n) {
            let at = (at - pos) as u32;
            events.push(&NoteOffEvent::new(at, Pckn::new(0u16, 0u16, r.key, 0u32), 0.0));
            events.push(&NoteOnEvent::new(at, Pckn::new(0u16, 0u16, key, 0u32), 1.0));
        }
        if let Some(at) = release.filter(|&at| at >= pos && at < pos + n) {
            events.push(&NoteOffEvent::new((at - pos) as u32, Pckn::new(0u16, 0u16, r.key, 0u32), 0.0));
        }
        block(&events, n, &mut bufs)?;
        for i in 0..n {
            samples.push(bufs[0][0][i]);
            samples.push(bufs[0][1][i]);
        }
        for (m, c) in mods.iter_mut().zip(bufs.iter().skip(1).flatten()) {
            m.extend_from_slice(&c[..n]);
        }
        pos += n;
    }
    drop(block);
    let processor = processor.stop_processing();
    instance.deactivate(processor);
    Ok(Audio { samples, mods })
}

/// Scenario results.
#[derive(Default)]
struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.push(name.to_string());
        }
    }

    /// A render is finite and audible. Peaks above 0 dBFS are fine: the plugin
    /// outputs float, the DAW mix does the gain staging.
    fn healthy(&mut self, name: &str, a: &Audio) {
        let rms = a.rms_db(0.0, a.secs());
        let peak = a.peak_db();
        self.check(name, a.finite() && rms > -45.0, format!("rms {rms:.1} dBFS, peak {peak:.1} dBFS"));
    }
}

/// A small library of synthesized files named like a user's: kicks,
/// snares, hats, a ride, a break and a mudpie.
fn make_library(dir: &Path) -> Result<(), Error> {
    use std::f64::consts::TAU;
    std::fs::create_dir_all(dir)?;
    let write = |name: &str, secs: f64, f: &dyn Fn(f64) -> f64| -> Result<(), Error> {
        let spec = hound::WavSpec { channels: 1, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(dir.join(name), spec)?;
        for i in 0..(44_100.0 * secs) as usize {
            w.write_sample((f(i as f64 / 44_100.0).clamp(-1.0, 1.0) * 32_000.0) as i16)?;
        }
        w.finalize()?;
        Ok(())
    };
    let noise = |t: f64, k: f64| ((t * 12_345.678 * k).sin() * 43_758.545).fract() * 2.0 - 1.0;
    for (i, f0) in [48.0, 55.0].iter().enumerate() {
        write(&format!("Kick {:02}.wav", i + 1), 0.4, &|t| (TAU * (f0 + 90.0 * (-t / 0.03).exp()) * t).sin() * (-t / 0.15).exp())?;
    }
    for i in 0..2 {
        let k = 1.0 + i as f64;
        write(&format!("Snare {:02}.wav", i + 1), 0.3, &|t| (0.6 * noise(t, k) + 0.4 * (TAU * 190.0 * t).sin()) * (-t / 0.07).exp())?;
    }
    write("Closed Hat 01.wav", 0.12, &|t| noise(t, 3.0) * (-t / 0.02).exp())?;
    write("Open Hat 01.wav", 0.6, &|t| noise(t, 4.0) * (-t / 0.2).exp())?;
    write("Ride 01.wav", 1.2, &|t| (noise(t, 5.0) * 0.4 + (TAU * 3_100.0 * t).sin() * 0.3) * (-t / 0.5).exp())?;
    let step = 60.0 / 170.0 / 4.0;
    write("170 Break 01.wav", 16.0 * step, &|t| {
        let s = (t / step) as usize;
        let local = t - s as f64 * step;
        match s % 16 {
            0 | 10 => (TAU * 55.0 * local).sin() * (-local / 0.1).exp(),
            4 | 12 => noise(t, 6.0) * (-local / 0.06).exp(),
            _ => noise(t, 7.0) * 0.3 * (-local / 0.015).exp(),
        }
    })?;
    write("Mudpie Reece F1.wav", 6.0, &|t| ((TAU * 43.65 * t).sin() + 0.5 * (TAU * 43.9 * t).sin() + 0.3 * (TAU * 87.4 * t).sin()) * 0.4)?;
    Ok(())
}

/// Loudness is calibrated (G5): every seed of both modes lands on the
/// target at the knobs' middle, so a new seed or sample never jumps in level.
fn loudness(entry: &PluginEntry, rep: &mut Report, base: &Render, what: &str) -> Result<(), Error> {
    const TARGET: f64 = -14.0;
    for mode in ["drums", "bass"] {
        let mut all = Vec::new();
        for seed in [1, 7, 21, 99] {
            let a = render(entry, &Render { mode, seed: Some(seed), bars: 16.0, ..base.clone() })?;
            all.push(a.lufs());
        }
        let lo = all.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = all.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let ok = all.iter().all(|l| (l - TARGET).abs() <= 1.5);
        let list: Vec<String> = all.iter().map(|l| format!("{l:.1}")).collect();
        rep.check(&format!("{what} {mode} land on {TARGET} LUFS over seeds"), ok, format!("{} LUFS (spread {:.1} dB)", list.join(", "), hi - lo));
    }
    Ok(())
}

/// Every scenario. Renders go to `out`.
fn suite(entry: &PluginEntry, library: Option<PathBuf>, out: &Path) -> Result<Report, Error> {
    std::fs::create_dir_all(out)?;
    let mut rep = Report::default();
    let save = |name: &str, a: &Audio| a.write(&out.join(format!("{name}.wav")));

    // Built-in sounds (no library).
    std::env::remove_var("MESH_VST_LIBRARY");
    // The warmup lets the background kit load and loudness calibration land.
    let base = Render { seed: Some(7), bars: 4.0, warmup: 1.5, ..Default::default() };
    let drums = render(entry, &base)?;
    save("builtin-drums", &drums)?;
    rep.healthy("built-in drums play", &drums);
    let bass = render(entry, &Render { mode: "bass", ..base.clone() })?;
    save("builtin-bass", &bass)?;
    rep.healthy("built-in bass plays", &bass);

    // The held note moves the bass (D7): another key after bar 2, an octave up.
    let half = bass.samples.len() / 2;
    let switched = render(entry, &Render { mode: "bass", switch: Some((2.0, 36)), ..base.clone() })?;
    rep.check(
        "a new key changes the bass from the switch on",
        switched.samples[..half - 2048] == bass.samples[..half - 2048] && switched.samples[half..] != bass.samples[half..],
        format!(
            "first difference at frame {:?} of {}, after the switch {}",
            switched.samples.iter().zip(&bass.samples).position(|(a, b)| a != b).map(|i| i / 2),
            half / 2,
            if switched.samples[half..] != bass.samples[half..] { "differs" } else { "the same" }
        ),
    );
    let octave = render(entry, &Render { mode: "bass", key: base.key + 12, ..base.clone() })?;
    rep.check("an octave up plays other notes", octave.samples != bass.samples, String::new());

    // The mod outputs (D32): 16 channels, levels 0..1, following the hits.
    let range = |a: &Audio| a.mods.iter().flatten().all(|v| (0.0..=1.0).contains(v));
    let peak = |a: &Audio, ch: usize| a.mods.get(ch).map_or(0.0, |c| c.iter().cloned().fold(0.0, f32::max));
    rep.check(
        "drums mod out: 16 channels in range, kick and snare light, the beat ramps",
        drums.mods.len() == 16 && range(&drums) && peak(&drums, 2) > 0.5 && peak(&drums, 3) > 0.3 && peak(&drums, 8) > 0.95,
        format!("{} channels, kick {:.2}, snare {:.2}, beat ramp {:.2}", drums.mods.len(), peak(&drums, 2), peak(&drums, 3), peak(&drums, 8)),
    );
    rep.check(
        "bass mod out: amp and filter envelopes, the LFO swings",
        bass.mods.len() == 16 && range(&bass) && peak(&bass, 2) > 0.3 && peak(&bass, 3) > 0.9 && peak(&bass, 4) > 0.9,
        format!("amp {:.2}, filter {:.2}, LFO {:.2}", peak(&bass, 2), peak(&bass, 3), peak(&bass, 4)),
    );

    let again = render(entry, &base)?;
    rep.check("same seed renders the same audio", again.samples == drums.samples, format!("{} frames", drums.frames()));
    let other = render(entry, &Render { seed: Some(8), ..base.clone() })?;
    rep.check("another seed renders another groove", other.samples != drums.samples, String::new());

    let stopped = render(entry, &Render { playing: false, ..base.clone() })?;
    rep.healthy("plays with the transport stopped", &stopped);

    let bar = 4.0 * 60.0 / base.bpm;
    let released = render(entry, &Render { release_at: Some(1.0), ..base.clone() })?;
    let (held, tail) = (released.rms_db(0.0, bar), released.rms_db(bar + 0.8, released.secs()));
    rep.check("a release goes silent after the tails", held > -45.0 && tail < -90.0, format!("before {held:.1} dBFS, after {tail:.1} dBFS"));

    // Every drum style plays and sounds different (D39).
    let mut styles: Vec<Audio> = Vec::new();
    for i in 1..=5 {
        let a = render(entry, &Render { params: vec![("Style".into(), i as f64)], ..base.clone() })?;
        rep.healthy(&format!("drum style {i} plays"), &a);
        styles.push(a);
    }
    // Swing, Morph and Permute are heard: their extremes differ by more than a trace.
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    for knob in ["Groove", "Morph", "Permute", "Tone", "Heat"] {
        let lo = render(entry, &Render { params: vec![(knob.into(), 0.0)], ..base.clone() })?;
        let hi = render(entry, &Render { params: vec![(knob.into(), 1.0)], ..base.clone() })?;
        let diff: Vec<f32> = lo.samples.iter().zip(&hi.samples).map(|(a, b)| a - b).collect();
        let db = 20.0 * (rms(&diff) / rms(&lo.samples).max(1e-9)).log10();
        rep.check(&format!("{knob} changes the drums"), db > -20.0, format!("difference {db:.1} dB below the drums"));
    }
    // The three release tails at full Shape sound apart.
    let mut tails = Vec::new();
    for i in 0..3 {
        tails.push(render(entry, &Render { params: vec![("Shape".into(), 1.0), ("Tail".into(), i as f64)], ..base.clone() })?);
    }
    let apart = (0..3).all(|i| (i + 1..3).all(|j| tails[i].samples != tails[j].samples));
    rep.check("the three release tails differ", apart, String::new());
    let distinct = (0..styles.len()).all(|i| (i + 1..styles.len()).all(|j| styles[i].samples != styles[j].samples));
    rep.check("the drum styles differ", distinct, format!("{} styles", styles.len()));

    loudness(entry, &mut rep, &base, "built-in")?;

    // A library, scanned and loaded in the background.
    let (lib, warmup) = match library {
        Some(dir) => (dir, 20.0),
        None => {
            let dir = out.join("library");
            make_library(&dir)?;
            (dir, 4.0)
        }
    };
    std::env::set_var("MESH_VST_LIBRARY", &lib);
    let lib_drums = render(entry, &Render { warmup, ..base.clone() })?;
    save("library-drums", &lib_drums)?;
    rep.healthy("library drums play", &lib_drums);
    rep.check("the library kit replaces the built-in one", lib_drums.samples != drums.samples, lib.display().to_string());
    let lib_bass = render(entry, &Render { mode: "bass", warmup: warmup.min(6.0), ..base.clone() })?;
    save("library-bass", &lib_bass)?;
    rep.healthy("library mudpie plays", &lib_bass);
    rep.check("the library mudpie replaces the built-in one", lib_bass.samples != bass.samples, String::new());
    // Every bar of every seed's mudpie is heard: no bar far below the others.
    let mut bars_db = Vec::new();
    for seed in 1..=8 {
        let a = render(entry, &Render { mode: "bass", seed: Some(seed), warmup: 2.0, ..base.clone() })?;
        let bar = 4.0 * 60.0 / base.bpm;
        for b in 0..base.bars as usize {
            bars_db.push(a.rms_db(b as f64 * bar, (b + 1) as f64 * bar));
        }
    }
    let mut sorted = bars_db.clone();
    sorted.sort_by(f64::total_cmp);
    let (low, median) = (sorted[0], sorted[sorted.len() / 2]);
    rep.check("every library mudpie bar is heard", low > median - 12.0, format!("quietest bar {low:.1} dBFS, median {median:.1} dBFS over 8 seeds"));
    // Library one-shots end: no voices pile up and ring on after the release
    // (a "kick" file can be a run of kicks).
    let mut tail = f64::NEG_INFINITY;
    for seed in [1, 2, 3, 4, 7443] {
        let a = render(entry, &Render { seed: Some(seed), bars: 8.0, release_at: Some(4.0), warmup: 2.0, ..base.clone() })?;
        let off = 4.0 * 4.0 * 60.0 / base.bpm;
        tail = tail.max(a.rms_db(off + 3.2, off + 4.0));
    }
    rep.check("library drums stop after the release", tail < -60.0, format!("loudest tail 3.2 s after the release {tail:.1} dBFS"));
    for bpm in [140.0, 174.0] {
        let a = render(entry, &Render { bpm, warmup: 2.0, ..base.clone() })?;
        rep.healthy(&format!("library drums at {bpm} bpm"), &a);
    }
    loudness(entry, &mut rep, &Render { warmup: 3.0, ..base.clone() }, "library")?;
    Ok(rep)
}

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().ok_or("usage: hostcheck suite|render <bundle.clap> [options]")?;
    let bundle = args.next().ok_or("missing bundle path")?;
    let mut opt: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Some(a) = args.next() {
        let name = a.strip_prefix("--").ok_or_else(|| format!("unexpected {a}"))?.to_string();
        if name == "stopped" {
            opt.entry(name).or_default().push(String::new());
            continue;
        }
        let v = args.next().ok_or_else(|| format!("--{name} needs a value"))?;
        opt.entry(name).or_default().push(v);
    }
    let get = |k: &str| opt.get(k).and_then(|v| v.last()).cloned();

    // The plugin's library cache stays out of the user's home unless asked.
    if std::env::var_os("XDG_CACHE_HOME").is_none() {
        std::env::set_var("XDG_CACHE_HOME", std::env::temp_dir().join("mesh-vst-hostcheck-cache"));
    }
    // The user's settings (library folders) never leak in: the library comes from --library.
    std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join("mesh-vst-hostcheck-config"));
    // Fixed starting values, so renders without --seed repeat.
    std::env::set_var("MESH_VST_FIXED_DEFAULTS", "1");
    // The plugin waits for each kit in `process`: no warmup on the clock.
    std::env::set_var("MESH_VST_SYNC_LOAD", "1");
    // SAFETY: loading a plugin runs its code; this tool is for our own bundle.
    let entry = unsafe { PluginEntry::load(&bundle)? };

    match cmd.as_str() {
        "suite" => {
            let out = PathBuf::from(get("out").unwrap_or_else(|| "hostcheck-out".into()));
            let rep = suite(&entry, get("library").map(PathBuf::from), &out)?;
            if rep.failures.is_empty() {
                println!("all scenarios passed; renders in {}", out.display());
                Ok(())
            } else {
                Err(format!("{} failed: {}", rep.failures.len(), rep.failures.join(", ")).into())
            }
        }
        "render" => {
            let mode = if get("mode").as_deref() == Some("bass") { "bass" } else { "drums" };
            let mut params = Vec::new();
            for p in opt.get("param").into_iter().flatten() {
                let (n, v) = p.split_once('=').ok_or("--param name=value")?;
                params.push((n.to_string(), v.parse()?));
            }
            let r = Render {
                mode,
                seed: get("seed").map(|s| s.parse()).transpose()?,
                bars: get("bars").map_or(Ok(4.0), |s| s.parse())?,
                bpm: get("bpm").map_or(Ok(172.0), |s| s.parse())?,
                key: get("key").map_or(Ok(29), |s| s.parse())?,
                playing: !opt.contains_key("stopped"),
                release_at: get("release-at").map(|s| s.parse()).transpose()?,
                switch: get("switch")
                    .map(|s| -> Result<(f64, u16), Error> {
                        let (b, k) = s.split_once(':').ok_or("--switch bar:key")?;
                        Ok((b.parse()?, k.parse()?))
                    })
                    .transpose()?,
                warmup: get("warmup").map_or(Ok(0.0), |s| s.parse())?,
                params,
            };
            let a = render(&entry, &r)?;
            let out = PathBuf::from(get("out").unwrap_or_else(|| "hostcheck.wav".into()));
            a.write(&out)?;
            println!("{}: peak {:.1} dBFS, rms {:.1} dBFS, finite {}", out.display(), a.peak_db(), a.rms_db(0.0, a.secs()), a.finite());
            Ok(())
        }
        c => Err(format!("unknown command {c}").into()),
    }
}
