//! Sorts a sample into a pool from its name and its sound (design D17, S1).
//!
//! No model and no training: filename keywords first, hand-set rules on a
//! few audio features otherwise, and "unsure" when the two disagree or the
//! features sit on a threshold. Measured on 1288 sorted samples with the
//! folder names hidden: 81.5 % right, 15.6 % unsure, 96.5 % of the answers
//! right (docs/design.md, S1). The thresholds come from that spike (its v2
//! rules).

use realfft::RealFftPlanner;

/// What the classifier decides a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Kick,
    Snare,
    Hat,
    Cymbal,
    Perc,
    Loop,
    Fx,
    Tonal,
}

impl Class {
    const ALL: [Class; 8] =
        [Class::Kick, Class::Snare, Class::Hat, Class::Cymbal, Class::Perc, Class::Loop, Class::Fx, Class::Tonal];

    /// Classes that are easy to confuse share a group; a name and a sound in
    /// different groups make the file unsure.
    fn group(self) -> u8 {
        match self {
            Class::Kick => 0,
            Class::Snare | Class::Perc => 1,
            Class::Hat | Class::Cymbal => 2,
            Class::Loop => 3,
            Class::Fx | Class::Tonal => 4,
        }
    }
}

/// Audio features of one file (from at most its first [`ANALYSE_SECS`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Features {
    /// Whole file, in seconds.
    pub dur: f32,
    pub peak: f32,
    /// Power below 150 Hz, above 6 kHz, and between 150 and 800 Hz, as shares.
    pub low: f32,
    pub hi: f32,
    pub body: f32,
    /// Spectral flatness 0..1 (noise near 1).
    pub flat: f32,
    /// Seconds from the peak to −30 dB.
    pub decay: f32,
    pub onsets: u32,
    /// Onsets per second.
    pub onset_rate: f32,
    /// Pitch clarity 0..1 (autocorrelation peak) and the share of frames on
    /// a steady pitch.
    pub clarity: f32,
    pub stable: f32,
    /// Fundamental of the steady pitch in Hz, 0 when there is none.
    pub pitch_hz: f32,
    /// Tempo estimate of a loop, 0 for shorter files.
    pub bpm: f32,
    pub ok: bool,
}

/// Seconds of audio the features read.
pub const ANALYSE_SECS: f32 = 8.0;

/// Classify by name alone: every class a keyword of `text` (a path or file
/// name) points to.
pub fn by_name(text: &str) -> Vec<Class> {
    let t = text.to_lowercase();
    let toks: Vec<&str> = t.split(|c: char| !c.is_ascii_alphabetic()).filter(|s| !s.is_empty()).collect();
    let tok = |k: &str| toks.iter().any(|x| *x == k);
    let sub = |k: &str| t.contains(k);
    let any_sub = |ks: &[&str]| ks.iter().any(|k| sub(k));
    let any_tok = |ks: &[&str]| ks.iter().any(|k| tok(k));
    let mut m = [false; 8];
    m[0] = any_sub(&["kick", "kik", "bassdrum"]) || any_tok(&["bd", "kck", "kk"]);
    m[1] = any_sub(&["snare", "snr", "rimshot", "clap"]) || any_tok(&["sd", "sn", "rim", "snap", "clp"]);
    m[2] = any_sub(&["hihat", "hi hat", "hi_hat"]) || any_tok(&["hat", "hats", "hh", "oh", "ch"]);
    m[3] = any_sub(&["crash", "cymbal", "splash", "ride", "shaker", "tambourine"]) || any_tok(&["cym", "china", "shake"]);
    m[4] = any_sub(&["perc", "cowbell", "bongo", "conga", "tambo", "clave"]) || any_tok(&["tom", "toms", "block"]);
    let brk = sub("break") || sub("amen");
    m[5] = sub("loop") || brk || any_tok(&["top", "tops"]);
    m[6] = any_sub(&["impact", "riser", "sweep", "foley", "atmos", "whoosh", "sfx", "uplifter", "downlifter", "reverse", "stab", "pluck", "misc"])
        || tok("fx");
    m[7] = any_sub(&["mudpie", "reece", "chord", "drone", "melo"]) || any_tok(&["synth", "synths", "pad", "pads", "bass", "lead", "keys"]);
    if sub("loop") {
        return vec![Class::Loop];
    }
    let instrument = m[0] || m[1] || m[2] || m[3];
    if brk && (instrument || m[4] || m[6]) {
        m[5] = false;
    }
    if m[4] && instrument {
        m[4] = false;
    }
    // A hat word wins over the cymbal words it shares a pool group with.
    if m[2] && m[3] && !any_sub(&["crash", "ride", "shaker"]) {
        m[3] = false;
    }
    Class::ALL.iter().zip(m).filter(|(_, on)| *on).map(|(c, _)| *c).collect()
}

/// Classify by sound alone. `scale` moves every threshold (1.0 = as set;
/// the caller tries 0.9 and 1.1 to find marginal files).
pub fn by_sound(f: &Features, scale: f32) -> Option<Class> {
    if !f.ok {
        return None;
    }
    let t = |v: f32| v * scale;
    if f.dur >= t(1.5) && f.onsets >= 4 && f.onset_rate >= t(1.5) {
        return Some(Class::Loop);
    }
    if f.low >= t(0.45) {
        return Some(if f.dur < t(2.5) && f.decay < t(0.6) { Class::Kick } else { Class::Tonal });
    }
    if f.hi >= t(0.45) && f.body < t(0.04) && f.low < 0.05 {
        return Some(if f.decay < t(0.3) && f.dur < t(1.0) { Class::Hat } else { Class::Cymbal });
    }
    if f.dur < t(1.2) && f.decay < t(0.6) && f.flat >= t(0.6) && f.hi >= t(0.05) && f.low < 0.3 {
        return Some(Class::Snare);
    }
    if f.dur < t(1.5) {
        return Some(Class::Perc);
    }
    if f.clarity >= t(0.7) && f.stable >= t(0.4) {
        return Some(Class::Tonal);
    }
    Some(Class::Fx)
}

/// Name and sound together. `None` = unsure.
pub fn classify(name: &str, f: &Features) -> Option<Class> {
    let names = by_name(name);
    let sound = by_sound(f, 1.0);
    let marginal = by_sound(f, 0.9) != sound || by_sound(f, 1.1) != sound;
    match names.len() {
        1 => match sound {
            Some(s) if !marginal && s.group() != names[0].group() => None,
            _ => Some(names[0]),
        },
        0 => {
            if marginal {
                None
            } else {
                sound
            }
        }
        _ => sound.filter(|s| names.contains(s)),
    }
}

/// Features of mono audio `x` at `sr`, `dur` seconds long in the file.
pub fn analyse(x0: &[f32], sr: u32, dur: f32) -> Features {
    let mut f = Features { dur, ..Default::default() };
    let peak = x0.iter().fold(0f32, |a, b| a.max(b.abs()));
    f.peak = peak;
    if peak < 1e-4 || x0.len() < 256 {
        return f;
    }
    let start = x0.iter().position(|v| v.abs() > 0.02 * peak).unwrap_or(0);
    let x = &x0[start..];
    let srf = sr as f32;

    // Envelope in dB, 512-frame windows every 256.
    let (win, hop) = (512usize, 256usize);
    let mut env = Vec::new();
    let mut i = 0;
    loop {
        let e = &x[i..(i + win).min(x.len())];
        let ms = e.iter().map(|v| v * v).sum::<f32>() / e.len() as f32;
        env.push(10.0 * (ms + 1e-12).log10());
        i += hop;
        if i >= x.len() {
            break;
        }
    }
    let (pi, pdb) = env.iter().enumerate().fold((0, -200f32), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
    f.decay = (env.len() - pi) as f32 * hop as f32 / srf;
    if let Some(j) = (pi + 1..env.len()).find(|&j| env[j] < pdb - 30.0) {
        f.decay = (j - pi) as f32 * hop as f32 / srf;
    }

    // Onsets: a rise of 6 dB within 5 frames, 80 ms apart.
    let refr = (0.08 * srf / hop as f32) as usize;
    let (mut last, mut count) = (0usize, 1u32);
    for j in 1..env.len() {
        let m = env[j.saturating_sub(5)..j].iter().cloned().fold(1e9, f32::min);
        if env[j] - m >= 6.0 && env[j] > pdb - 35.0 && j - last >= refr {
            count += 1;
            last = j;
        }
    }
    f.onsets = count;
    f.onset_rate = count as f32 / (x.len() as f32 / srf).max(0.05);

    // Tempo from the onset strength's autocorrelation, 80..190 bpm.
    if dur >= 1.5 && env.len() > 200 {
        let o: Vec<f32> = (1..env.len()).map(|j| (env[j] - env[j - 1]).max(0.0)).collect();
        let fps = srf / hop as f32;
        let (l0, l1) = ((fps * 60.0 / 190.0) as usize, (fps * 60.0 / 80.0) as usize);
        if l1 + 1 < o.len() {
            let acf = |l: usize| (0..o.len() - l).map(|k| o[k] * o[k + l]).sum::<f32>() / (o.len() - l) as f32;
            let best = (l0..=l1).map(|l| (l, acf(l))).fold((l0, -1.0), |a, b| if b.1 > a.1 { b } else { a });
            f.bpm = 60.0 * fps / best.0 as f32;
        }
    }

    // Average spectrum, 2048-point Hann frames with 50 % overlap.
    let n = 2048usize;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let hann: Vec<f32> = (0..n).map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / n as f32).cos()).collect();
    let (mut mag, mut pw) = (vec![0f32; n / 2 + 1], vec![0f32; n / 2 + 1]);
    let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    let mut p = 0usize;
    loop {
        for k in 0..n {
            buf[k] = x.get(p + k).map_or(0.0, |v| v * hann[k]);
        }
        if fft.process(&mut buf, &mut spec).is_err() {
            break;
        }
        for k in 0..=n / 2 {
            let m = spec[k].norm();
            mag[k] += m;
            pw[k] += m * m;
        }
        p += n / 2;
        if p >= x.len() {
            break;
        }
    }
    let bin = srf / n as f32;
    let total: f32 = pw.iter().sum::<f32>().max(1e-20);
    let k150 = (150.0 / bin) as usize;
    let k800 = (800.0 / bin) as usize;
    let k6k = ((6000.0 / bin) as usize).min(n / 2);
    f.low = pw[..k150].iter().sum::<f32>() / total;
    f.hi = pw[k6k..].iter().sum::<f32>() / total;
    f.body = pw[k150..k800].iter().sum::<f32>() / total;
    let (a, b) = ((100.0 / bin) as usize, ((16000.0 / bin) as usize).min(n / 2));
    let band = &mag[a..b];
    let log_mean = band.iter().map(|v| (v + 1e-12).ln()).sum::<f32>() / band.len() as f32;
    f.flat = log_mean.exp() / (band.iter().sum::<f32>() / band.len() as f32 + 1e-12);

    // Pitch clarity on the signal decimated by 4.
    let y: Vec<f32> = x.chunks_exact(4).map(|c| c.iter().sum::<f32>() / 4.0).collect();
    let ysr = srf / 4.0;
    let (fl, fh) = (1024usize, 512usize);
    if y.len() > fl {
        let mut frames = Vec::new();
        let mut s = 0;
        while s + fl <= y.len() {
            let e = y[s..s + fl].iter().map(|v| v * v).sum::<f32>();
            frames.push((s, 10.0 * (e / fl as f32 + 1e-12).log10()));
            s += fh;
        }
        let loudest = frames.iter().map(|f| f.1).fold(-200f32, f32::max);
        let active: Vec<usize> = frames.iter().filter(|f| f.1 > loudest - 30.0).map(|f| f.0).collect();
        let step = (active.len() / 24).max(1);
        let mut picks: Vec<(f32, usize)> = Vec::new();
        for &s in active.iter().step_by(step) {
            let fr = &y[s..s + fl];
            // Local peaks of the normalized autocorrelation. Multiples of the
            // period peak as high as the period itself, so the period is the
            // shortest lag within 10 % of the best peak.
            let (mut peaks, mut prev, mut prev2) = (Vec::new(), 0f32, 0f32);
            for l in 8..=280usize {
                let (mut r, mut e0, mut el) = (0f32, 0f32, 0f32);
                for i in 0..fl - l {
                    r += fr[i] * fr[i + l];
                    e0 += fr[i] * fr[i];
                    el += fr[i + l] * fr[i + l];
                }
                let v = r / (e0 * el).sqrt().max(1e-12);
                if prev > prev2 && prev >= v && l > 9 {
                    peaks.push((prev, l - 1));
                }
                prev2 = prev;
                prev = v;
            }
            let best = peaks.iter().map(|p| p.0).fold(0f32, f32::max);
            let lag = peaks.iter().find(|p| p.0 >= 0.9 * best).map_or(0, |p| p.1);
            picks.push((best, lag));
        }
        if !picks.is_empty() {
            let mut cs: Vec<f32> = picks.iter().map(|c| c.0).collect();
            cs.sort_by(|a, b| a.total_cmp(b));
            f.clarity = cs[cs.len() / 2];
            let mut lags: Vec<usize> = picks.iter().filter(|c| c.0 > 0.7).map(|c| c.1).collect();
            lags.sort_unstable();
            if let Some(&med) = lags.get(lags.len() / 2) {
                let med = med as f32;
                let steady = picks.iter().filter(|c| c.0 > 0.7 && ((c.1 as f32 - med).abs() / med) < 0.04).count();
                f.stable = steady as f32 / picks.len() as f32;
                f.pitch_hz = ysr / med;
            }
        }
    }
    f.ok = true;
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_point_to_their_pools() {
        assert_eq!(by_name("Drums/001_Kick_-_CRUNCHDNB.wav"), vec![Class::Kick]);
        assert_eq!(by_name("SN tight 07.wav"), vec![Class::Snare]);
        assert_eq!(by_name("001 Hi Hat - DECODED.wav"), vec![Class::Hat]);
        assert_eq!(by_name("ride_bell_01.wav"), vec![Class::Cymbal]);
        assert_eq!(by_name("shaker_loose.wav"), vec![Class::Cymbal]);
        assert_eq!(by_name("170_ThursdayBreak_SP_01.wav"), vec![Class::Loop]);
        assert_eq!(by_name("Mudpies/Metally Reece.wav"), vec![Class::Tonal]);
        assert_eq!(by_name("hat loop 03.wav"), vec![Class::Loop]);
        assert!(by_name("Sample 01.wav").is_empty());
    }

    fn sr() -> u32 {
        48_000
    }

    #[test]
    fn sounds_sort_without_names() {
        let sr = sr();
        let kick: Vec<f32> = (0..sr / 3)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (std::f32::consts::TAU * (45.0 + 80.0 * (-t / 0.03).exp()) * t).sin() * (-t / 0.1).exp()
            })
            .collect();
        let f = analyse(&kick, sr, kick.len() as f32 / sr as f32);
        assert_eq!(classify("Sample 01.wav", &f), Some(Class::Kick), "{f:?}");

        let mut x = 7u32;
        let hat: Vec<f32> = (0..sr / 10)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 - 0.5) * (-(i as f32) / sr as f32 / 0.02).exp()
            })
            .collect();
        let hp: Vec<f32> = hat.windows(2).map(|w| w[1] - w[0]).collect();
        let f = analyse(&hp, sr, hp.len() as f32 / sr as f32);
        assert_eq!(by_sound(&f, 1.0), Some(Class::Hat), "{f:?}");

        let tone: Vec<f32> = (0..sr * 3).map(|i| (std::f32::consts::TAU * 87.3 * i as f32 / sr as f32).sin() * 0.5).collect();
        let f = analyse(&tone, sr, 3.0);
        assert_eq!(by_sound(&f, 1.0), Some(Class::Tonal), "{f:?}");
        assert!((f.pitch_hz - 87.3).abs() < 3.0, "{}", f.pitch_hz);
    }

    #[test]
    fn a_name_against_its_sound_is_unsure() {
        let sr = sr();
        let tone: Vec<f32> = (0..sr * 3).map(|i| (std::f32::consts::TAU * 87.3 * i as f32 / sr as f32).sin() * 0.5).collect();
        let f = analyse(&tone, sr, 3.0);
        assert_eq!(classify("hihat_01.wav", &f), None);
        assert_eq!(classify("reece_bass.wav", &f), Some(Class::Tonal));
    }
}
