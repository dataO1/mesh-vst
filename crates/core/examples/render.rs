//! Offline render of one lane to a WAV, for listening tests.
//!
//! ```text
//! cargo run -p mesh-vst-core --release --example render -- \
//!   --mode drums --seed 7 --bars 16 --bpm 172 --out drums.wav \
//!   --kick k.wav --snare s.wav --hat h.wav --open oh.wav --break amen.wav
//! cargo run -p mesh-vst-core --release --example render -- \
//!   --mode bass --seed 7 --bars 16 --key 41 --out bass.wav --mudpie pie.wav --root 41
//! ```
//!
//! `--form N` plays the form seed N builds (all three form seeds N), or a
//! form as written; `--param name=value` sets a generator parameter;
//! `--neuro 0..1` runs the bass's neuro chain (D53) on the output,
//! `--timbre centroid,bright,sub,over_1k` gives it a scanned file's
//! spectrum (at `--root`).


use std::path::PathBuf;

use mesh_vst_core::form::Form;
use mesh_vst_core::macros::FormSettings;
use mesh_vst_core::gc::gc_handle;
use mesh_vst_core::kit::{grid_slices, mudpie_bank, share, DrumKit};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::player::{Player, Transport};
use mesh_vst_core::sampler::{SampleData, Sampler};
use mesh_vst_core::types::StereoSample;

const SR: u32 = 48_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut opt = std::collections::BTreeMap::<String, Vec<String>>::new();
    while let Some(a) = args.next() {
        let Some(name) = a.strip_prefix("--") else { fail(&format!("unexpected argument {a}")) };
        let value = args.next().unwrap_or_else(|| fail(&format!("--{name} needs a value")));
        opt.entry(name.to_string()).or_default().push(value);
    }
    let one = |k: &str| opt.get(k).and_then(|v| v.last()).cloned();
    let num = |k: &str, d: f64| one(k).map_or(d, |v| v.parse().unwrap_or_else(|_| fail(&format!("--{k}: not a number"))));
    let load = |k: &str, root: u8| {
        one(k).map(|p| {
            let data = SampleData::load_file(&PathBuf::from(&p), root, SR).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
            share(data)
        })
    };

    let seed = num("seed", 1.0) as u32;
    let sound = num("sound", seed as f64) as u32;
    let bars = num("bars", 8.0) as u32;
    let bpm = num("bpm", 172.0);
    let key = num("key", 41.0) as u8;
    let out_path = one("out").unwrap_or_else(|| "render.wav".into());

    let mode = one("mode").unwrap_or_else(|| "drums".into());
    let (lane, bank, voices) = match mode.as_str() {
        "drums" => {
            let brk = one("break").map(|p| {
                let data = SampleData::load_file(&PathBuf::from(&p), 60, SR).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
                share(grid_slices(data, num("slices", 16.0) as usize))
            });
            let kit = DrumKit {
                kick: load("kick", 60),
                snare: load("snare", 60),
                hats_closed: load("hat", 60).into_iter().collect(),
                hats_open: load("open", 60).into_iter().collect(),
                rides: load("ride", 60).into_iter().collect(),
                perc: load("perc", 60).into_iter().collect(),
                break_loop: brk,
                ..Default::default()
            };
            let (layout, bank) = kit.build();
            (PatternLane::drums(layout, seed, sound), bank, 48)
        }
        "bass" => {
            let pie = load("mudpie", num("root", 41.0) as u8).unwrap_or_else(|| fail("--mudpie is required"));
            (PatternLane::bass(seed, sound), mudpie_bank(pie), 16)
        }
        m => fail(&format!("unknown mode {m}")),
    };

    let mut sampler = Sampler::new(SR, voices);
    sampler.set_bank(basedrop::Shared::new(&gc_handle(), bank));
    let mut player = Player::new(lane, sampler);
    if let Some(s) = one("form") {
        // A seed builds a form (D48), anything else is a form as written.
        match s.parse::<u32>() {
            Ok(seed) => {
                let drums = player.lane().role() == mesh_vst_core::role::Role::Drums;
                player.lane_mut().set_form_spec(FormSettings::seeded(seed).spec(drums, 0.5, 0.5, 0.5));
            }
            Err(_) => player.lane_mut().set_form(Form::parse(&s).unwrap_or_else(|e| fail(&format!("form: {e:?}")))),
        }
    }
    for p in opt.get("param").into_iter().flatten() {
        let (name, value) = p.split_once('=').unwrap_or_else(|| fail("--param name=value"));
        let value: f32 = value.parse().unwrap_or_else(|_| fail("--param: value not a number"));
        player.lane_mut().set_param(name, value);
    }

    player.note_on(key, 1.0, 0.0);
    let total = (bars as f64 * 4.0 * 60.0 / bpm * SR as f64) as usize;
    let spec = hound::WavSpec { channels: 2, sample_rate: SR, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut wav = hound::WavWriter::create(&out_path, spec).unwrap_or_else(|e| fail(&format!("{out_path}: {e}")));
    let mut buf = vec![StereoSample::silence(); 256];
    let neuro_amount = num("neuro", 0.0) as f32;
    let mut neuro = mesh_vst_core::neuro::Neuro::new(SR as f32);
    let neuro_knobs = mesh_vst_core::neuro::NeuroKnobs { amount: neuro_amount, shape: 0.3, morph: 0.5, variation: 0.5, spread: 0.5, contrast: 0.5, sound_seed: sound, letter: 0, timbre: one("timbre").map(|t| {
        let v: Vec<f32> = t.split(',').map(|x| x.parse().unwrap_or_else(|_| fail("--timbre centroid,bright"))).collect();
        let at = |i: usize| v.get(i).copied().unwrap_or(0.0);
        (mesh_vst_core::library::Timbre { centroid: at(0), bright: at(1), flat: 0.0, sub: at(2), over_1k: at(3), version: mesh_vst_core::library::TIMBRE_VERSION }, num("root", 41.0) as u8)
    }) };
    let mut pos = 0;
    let mut peak = 0.0f32;
    while pos < total {
        let n = 256.min(total - pos);
        let beat = pos as f64 * bpm / 60.0 / SR as f64;
        player.process(&mut buf[..n], &Transport { playing: true, beat, bpm });
        if neuro_amount > 0.0 {
            let letter = player.rendered().map_or(0, |(_, b)| b.letter);
            neuro.process(&mut buf[..n], player.fired(), &mesh_vst_core::neuro::NeuroKnobs { letter, ..neuro_knobs }, bpm / 60.0 / SR as f64);
        }
        for s in &buf[..n] {
            peak = peak.max(s.left.abs()).max(s.right.abs());
            wav.write_sample(s.left).and_then(|_| wav.write_sample(s.right)).unwrap_or_else(|e| fail(&e.to_string()));
        }
        pos += n;
    }
    wav.finalize().unwrap_or_else(|e| fail(&e.to_string()));
    println!("{out_path}: {bars} bars at {bpm} bpm, seed {seed}, peak {:.1} dBFS", 20.0 * peak.max(1e-9).log10());
}

fn fail(msg: &str) -> ! {
    eprintln!("render: {msg}");
    std::process::exit(2)
}
