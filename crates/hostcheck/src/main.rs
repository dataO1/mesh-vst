//! Headless CLAP host for end-to-end checks of the bundled plugin.
//!
//! ```text
//! cargo run -p hostcheck --release -- "target/bundled/mesh vst.clap" \
//!   --mode bass --seed 7 --bars 8 --bpm 172 --key 41 --out bass.wav [--stopped] [--release-at 4]
//! ```
//!
//! It holds `--key` from the first block, sends the transport every block
//! (playing, unless `--stopped`), sets parameters by name (`--param
//! "Pattern seed=7"`), and writes the plugin's stereo output. With
//! `--release-at N` the note ends at bar N.

use std::collections::BTreeMap;

use clack_extensions::params::{ParamInfoBuffer, PluginParams};
use clack_host::events::event_types::{NoteOffEvent, NoteOnEvent, ParamValueEvent, TransportEvent, TransportFlags};
use clack_host::events::EventFlags;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};

const PLUGIN_ID: &[u8] = b"io.github.dataO1.mesh-vst";
const SR: f64 = 48_000.0;
const BLOCK: usize = 512;

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let bundle = args.next().ok_or("usage: hostcheck <bundle.clap> [options]")?;
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
    let get = |k: &str, d: &str| opt.get(k).and_then(|v| v.last()).cloned().unwrap_or_else(|| d.to_string());
    let bars: f64 = get("bars", "4").parse()?;
    let bpm: f64 = get("bpm", "172").parse()?;
    let key: u16 = get("key", "41").parse()?;
    let out_path = get("out", "hostcheck.wav");
    let playing = !opt.contains_key("stopped");
    let release_at: Option<f64> = opt.get("release-at").and_then(|v| v.last()).map(|s| s.parse()).transpose()?;

    let mut set: Vec<(String, f64)> = Vec::new();
    match get("mode", "drums").as_str() {
        "drums" => set.push(("Mode".into(), 0.0)),
        "bass" => set.push(("Mode".into(), 1.0)),
        m => return Err(format!("unknown mode {m}").into()),
    }
    if let Some(seed) = opt.get("seed").and_then(|v| v.last()) {
        set.push(("Pattern seed".into(), seed.parse()?));
        set.push(("Sound seed".into(), seed.parse()?));
    }
    for p in opt.get("param").into_iter().flatten() {
        let (n, v) = p.split_once('=').ok_or("--param name=value")?;
        set.push((n.to_string(), v.parse()?));
    }

    let host_info = HostInfo::new("hostcheck", "mesh vst", "https://github.com/dataO1/mesh-vst", "0.1")?;
    // SAFETY: loading a plugin runs its code; this tool is for our own bundle.
    let entry = unsafe { PluginEntry::load(&bundle)? };
    let factory = entry.get_plugin_factory().ok_or("no plugin factory")?;
    let descriptor = factory
        .plugin_descriptors()
        .find(|d| d.id().is_some_and(|id| id.to_bytes() == PLUGIN_ID))
        .ok_or("mesh vst not in the bundle")?;
    let id = descriptor.id().ok_or("no id")?;
    let mut instance = PluginInstance::<Host>::new(|_| Shared, |_| (), &entry, id, &host_info)?;

    // Parameter ids by name.
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
    let mut first_events = Vec::new();
    for (name, value) in &set {
        let id = *ids.get(name).ok_or_else(|| format!("no parameter {name}; have {:?}", ids.keys().collect::<Vec<_>>()))?;
        first_events.push(ParamValueEvent::new(0, id, Pckn::match_all(), *value));
    }

    let config = PluginAudioConfiguration { sample_rate: SR, min_frames_count: 1, max_frames_count: BLOCK as u32 };
    let processor = instance.activate(|_, _| (), config)?;
    let mut processor = processor.start_processing()?;

    let mut out_ports = AudioPorts::with_capacity(2, 1);
    let mut out_l = vec![0.0f32; BLOCK];
    let mut out_r = vec![0.0f32; BLOCK];
    let mut out_events = EventBuffer::new();
    let total = (bars * 4.0 * 60.0 / bpm * SR) as usize;
    let release_frame = release_at.map(|b| (b * 4.0 * 60.0 / bpm * SR) as usize);
    let spec = hound::WavSpec { channels: 2, sample_rate: SR as u32, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut wav = hound::WavWriter::create(&out_path, spec)?;
    let (mut peak, mut sum_sq, mut nan) = (0.0f32, 0.0f64, false);

    let mut pos = 0usize;
    while pos < total {
        let n = BLOCK.min(total - pos);
        let beat = if playing { pos as f64 / SR * bpm / 60.0 } else { 0.0 };
        let mut events = EventBuffer::new();
        events.push(&transport(beat, bpm, playing));
        if pos == 0 {
            for e in &first_events {
                events.push(e);
            }
            events.push(&NoteOnEvent::new(0, Pckn::new(0u16, 0u16, key, 0u32), 1.0));
        }
        if let Some(r) = release_frame.filter(|&r| r >= pos && r < pos + n) {
            events.push(&NoteOffEvent::new((r - pos) as u32, Pckn::new(0u16, 0u16, key, 0u32), 0.0));
        }
        let input = InputAudioBuffers::empty();
        let mut output = out_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only([&mut out_l[..n], &mut out_r[..n]].into_iter()),
        }]);
        out_events.clear();
        processor.process(&input, &mut output, &events.as_input(), &mut out_events.as_output(), None, None)?;
        for i in 0..n {
            let (l, r) = (out_l[i], out_r[i]);
            nan |= !l.is_finite() || !r.is_finite();
            peak = peak.max(l.abs()).max(r.abs());
            sum_sq += (l as f64).powi(2) + (r as f64).powi(2);
            wav.write_sample(l)?;
            wav.write_sample(r)?;
        }
        pos += n;
    }
    wav.finalize()?;
    let processor = processor.stop_processing();
    instance.deactivate(processor);
    let rms = (sum_sq / (2.0 * total as f64)).sqrt();
    println!(
        "{out_path}: {bars} bars at {bpm} bpm, {}, peak {:.1} dBFS, rms {:.1} dBFS{}",
        if playing { "transport playing" } else { "transport stopped" },
        20.0 * (peak.max(1e-9) as f64).log10(),
        20.0 * rms.max(1e-12).log10(),
        if nan { ", NON-FINITE SAMPLES" } else { "" }
    );
    Ok(())
}
