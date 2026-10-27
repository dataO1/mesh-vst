//! Synthesized sounds the plugin plays before a library is set up (M2), so
//! it makes sound in any DAW right after loading.

use mesh_vst_core::kit::{grid_slices, share, DrumKit};
use mesh_vst_core::loader::{slot_offset_db, Slot, SAMPLE_LOUDNESS_TARGET};
use mesh_vst_core::sampler::SampleData;

/// MIDI key the built-in mudpie sounds at (F2).
pub const MUDPIE_ROOT: u8 = 41;

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// A mono sample whose notes play at the library's loudness for `slot` (D64).
fn mono(s: Vec<f32>, sr: u32, root: u8, slot: Slot) -> SampleData {
    SampleData::from_channels(&s, &s, sr, root).with_note_lufs((SAMPLE_LOUDNESS_TARGET + slot_offset_db(slot)) as f32)
}

fn kick(sr: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.45) as usize;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let f = 45.0 + 110.0 * (-t / 0.035).exp();
            phase += f / sr as f32;
            let click = if t < 0.004 { (1.0 - t / 0.004) * 0.3 } else { 0.0 };
            ((std::f32::consts::TAU * phase).sin() * (-t / 0.18).exp() + click).tanh() * 0.9
        })
        .collect()
}

fn snare(sr: u32, seed: u32) -> Vec<f32> {
    let n = (sr as f32 * 0.3) as usize;
    let mut noise = Noise(seed);
    let mut lp = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let x = noise.next();
            lp += 0.35 * (x - lp);
            let body = (std::f32::consts::TAU * 190.0 * t).sin() * (-t / 0.05).exp();
            (0.6 * (x - lp) * (-t / 0.09).exp() + 0.5 * body) * 0.8
        })
        .collect()
}

fn hat(sr: u32, seed: u32, decay: f32) -> Vec<f32> {
    let n = (sr as f32 * (decay * 5.0).min(0.6)) as usize;
    let mut noise = Noise(seed);
    let mut prev = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let x = noise.next();
            // A first difference: a crude highpass.
            let y = x - prev;
            prev = x;
            0.35 * y * (-t / decay).exp()
        })
        .collect()
}

/// One bar of a two-step break at 170 bpm, from the hits above.
fn break_loop(sr: u32) -> Vec<f32> {
    let step = (sr as f64 * 60.0 / 170.0 / 4.0) as usize;
    let mut out = vec![0.0f32; step * 16];
    let k = kick(sr);
    let s = snare(sr, 11);
    let h = hat(sr, 13, 0.03);
    let mut mix = |src: &[f32], at: usize, gain: f32| {
        for (o, x) in out[at * step..].iter_mut().zip(src) {
            *o += x * gain;
        }
    };
    for st in [0, 10] {
        mix(&k, st, 0.9);
    }
    for st in [4, 12] {
        mix(&s, st, 0.9);
    }
    for st in [7, 9, 15] {
        mix(&s, st, 0.3);
    }
    for st in (0..16).step_by(2) {
        mix(&h, st, 0.6);
    }
    out.iter_mut().for_each(|x| *x = x.tanh());
    out
}

/// The built-in drum kit.
pub fn drum_kit(sr: u32) -> DrumKit {
    DrumKit {
        kick: Some(share(mono(kick(sr), sr, 60, Slot::Kick))),
        snare: Some(share(mono(snare(sr, 7), sr, 60, Slot::Snare))),
        hats_closed: vec![share(mono(hat(sr, 3, 0.025), sr, 60, Slot::HatClosed))],
        hats_open: vec![share(mono(hat(sr, 5, 0.12), sr, 60, Slot::HatOpen))],
        rides: vec![share(mono(hat(sr, 17, 0.3), sr, 60, Slot::CymbalA))],
        perc: vec![share(mono(snare(sr, 23).into_iter().map(|x| x * 0.5).collect(), sr, 60, Slot::Perc))],
        break_loop: Some(share(grid_slices(mono(break_loop(sr), sr, 60, Slot::Break), 16))),
        ..Default::default()
    }
}

/// The built-in mudpie: eight seconds of a detuned saw stack whose
/// brightness and detune drift, so each note position sounds different.
pub fn mudpie(sr: u32) -> SampleData {
    let n = sr as usize * 8;
    let f0 = 440.0 * 2f32.powf((MUDPIE_ROOT as f32 - 69.0) / 12.0);
    let detune = [-0.11f32, -0.04, 0.0, 0.05, 0.12];
    let mut phases = [0.0f32; 5];
    let mut lp = 0.0f32;
    let (mut left, mut right) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let drift = 1.0 + 0.5 * (t * 0.37).sin();
        let mut l = 0.0;
        let mut r = 0.0;
        for (k, (ph, d)) in phases.iter_mut().zip(detune).enumerate() {
            let f = f0 * 2f32.powf(d * drift / 12.0);
            *ph = (*ph + f / sr as f32).fract();
            let saw = 2.0 * *ph - 1.0;
            let pan = k as f32 / 4.0;
            l += saw * (1.0 - pan);
            r += saw * pan;
        }
        let bright = 0.05 + 0.25 * (0.5 + 0.5 * (t * 0.61).sin());
        lp += bright * ((l + r) * 0.5 - lp);
        let side = (l - r) * 0.08;
        left.push(((lp + side) * 0.45).tanh());
        right.push(((lp - side) * 0.45).tanh());
    }
    SampleData::from_channels(&left, &right, sr, MUDPIE_ROOT).with_note_lufs(SAMPLE_LOUDNESS_TARGET as f32)
}
