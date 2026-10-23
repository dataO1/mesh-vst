//! Output loudness calibration, as Mesh live's style calibration did it
//! (design G5).
//!
//! Mesh rendered every sound through its lane's own generator at the
//! default controls, measured the integrated loudness (BS.1770) and stored
//! the trim that brings it to its role's target. Here the same runs when a
//! kit loads or the seeds or form change: the lane plays offline with its
//! knobs at the seed's suggestion, and the trim it gets brings it to
//! [`target_lufs`]. Moving a knob then changes the loudness as it should, but
//! a new seed or sample never jumps in level.

use basedrop::Shared;

use crate::loudness::measure_lufs;
use crate::macros::{configure, Knobs, LaneSettings};
use crate::pattern::PatternLane;
use crate::player::{Player, Transport};
use crate::role::Role;
use crate::sampler::{LaneSamplerParams, SampleBank, Sampler};
use crate::types::StereoSample;

// MUSICAL: Mesh live's role targets (drums −12, bass −14 LUFS at the lane),
// 2 dB lower for drums so a kit's peaks keep below full scale.
/// Integrated loudness a lane is trimmed to.
pub fn target_lufs(role: Role) -> f64 {
    match role {
        Role::Drums => -14.0,
        _ => -14.0,
    }
}

/// Bars rendered for a measurement, its tempo and the held key.
const BARS: u32 = 16;
const BPM: f64 = 172.0;
const KEY: u8 = 29;
const BLOCK: usize = 512;
/// Largest cut and boost of a trim.
const TRIM_RANGE_DB: (f64, f64) = (-48.0, 48.0);

/// Integrated loudness of `lane` on `bank`, playing `settings` with its
/// knobs at the seed's suggestion. `-inf` when it stays silent.
pub fn lane_lufs(
    lane: PatternLane,
    bank: Shared<SampleBank>,
    base: LaneSamplerParams,
    settings: &LaneSettings,
    loop_bpm: Option<f64>,
    sample_rate: u32,
    voices: usize,
) -> f64 {
    render_lufs(lane, bank, LaneSamplerParams { gain: 1.0, ..base }, settings, loop_bpm, sample_rate, voices, false)
}

/// The trim that brings a bass lane to its target after the plugin's last
/// stages too (Heat at the centred knobs, the Squash at the settings'): they
/// keep the level only roughly for some material (a sustained clip, D50), so
/// the lane is measured raw, then once more through them at that trim.
pub fn bass_trim(
    lane: impl Fn() -> PatternLane,
    bank: Shared<SampleBank>,
    base: LaneSamplerParams,
    settings: &LaneSettings,
    sample_rate: u32,
    voices: usize,
) -> f32 {
    let gain = trim_gain(Role::Bass, lane_lufs(lane(), bank.clone(), base, settings, None, sample_rate, voices));
    let post = render_lufs(lane(), bank, LaneSamplerParams { gain, ..base }, settings, None, sample_rate, voices, true);
    if !post.is_finite() {
        return gain;
    }
    let db = (20.0 * (gain as f64).log10() + target_lufs(Role::Bass) - post).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1);
    10f64.powf(db / 20.0) as f32
}

/// [`lane_lufs`] at the gain `base` holds; `post`: through the bass's last
/// stages as well.
#[allow(clippy::too_many_arguments)]
fn render_lufs(
    mut lane: PatternLane,
    bank: Shared<SampleBank>,
    base: LaneSamplerParams,
    settings: &LaneSettings,
    loop_bpm: Option<f64>,
    sample_rate: u32,
    voices: usize,
    post: bool,
) -> f64 {
    let centred = LaneSettings { knobs: Knobs::CENTERED, ..*settings };
    let params = configure(&mut lane, base, &centred);
    let mut sampler = Sampler::new(sample_rate, voices);
    sampler.set_bank(bank);
    sampler.set_params(params);
    let drums = lane.role() == Role::Drums;
    let mut player = Player::new(lane, sampler);
    player.loop_bpm = loop_bpm;
    // The drums' post stages (release tail, ring modulator) at the centred
    // knobs, as the plugin plays them. (Heat keeps its level by itself.)
    let resolved = crate::macros::Centers::of(Role::Drums, settings.pattern_seed, settings.sound_seed).resolve(&Knobs::CENTERED);
    let mut tails = crate::tail::VoiceTails::new(sample_rate as f32);
    let mut ring = crate::heat::RingMod::new(sample_rate as f32);
    let (ring_hz, ring_mix, ring_shape) = crate::macros::ring(settings.sound_seed, resolved.morph);
    let tail_mode = crate::tail::TailMode::seeded(settings.sound_seed);
    let heat_amount = crate::macros::Centers::of(Role::Bass, settings.pattern_seed, settings.sound_seed).resolve(&Knobs::CENTERED).heat;
    let mut heat = crate::heat::Heat::new(sample_rate as f32);
    let mut squash = crate::squash::Squash::new(sample_rate as f32);
    player.note_on(KEY, 1.0, 0.0);
    let total = (BARS as f64 * 4.0 * 60.0 / BPM * sample_rate as f64) as usize;
    let mut out = Vec::with_capacity(total);
    let mut buf = vec![StereoSample::silence(); BLOCK];
    let mut buses = vec![StereoSample::silence(); crate::tail::GROUPS * BLOCK];
    let mut pos = 0;
    while pos < total {
        let n = BLOCK.min(total - pos);
        let beat = pos as f64 * BPM / 60.0 / sample_rate as f64;
        let transport = Transport { playing: true, beat, bpm: BPM };
        if drums {
            let buses = &mut buses[..crate::tail::GROUPS * n];
            player.process_split(&mut buf[..n], Some(&mut *buses), &transport);
            tails.process(&mut buf[..n], buses, resolved.shape, tail_mode, pos as u64, crate::tempo::stretch(BPM));
            ring.process(&mut buf[..n], ring_hz, ring_mix, ring_shape, pos as f64 / sample_rate as f64);
        } else {
            player.process(&mut buf[..n], &transport);
            if post {
                // The bass's order (D53): Squash, then Heat last.
                squash.process(&mut buf[..n], settings.squash, false, crate::tempo::stretch(BPM));
                heat.process(&mut buf[..n], heat_amount, 0.5, false);
            }
        }
        out.extend_from_slice(&buf[..n]);
        pos += n;
    }
    measure_lufs(&out, sample_rate as f64)
}

/// Linear output gain that brings a lane measured at `lufs` to its target.
pub fn trim_gain(role: Role, lufs: f64) -> f32 {
    if !lufs.is_finite() {
        return 1.0;
    }
    let db = (target_lufs(role) - lufs).clamp(TRIM_RANGE_DB.0, TRIM_RANGE_DB.1);
    10f64.powf(db / 20.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc::gc_handle;
    use crate::kit::{mudpie_bank, share};
    use crate::macros::mudpie_params;
    use crate::sampler::SampleData;

    fn pie(amp: f32) -> Shared<SampleBank> {
        let sr = 48_000;
        let s: Vec<f32> = (0..sr * 4).map(|i| (std::f32::consts::TAU * 87.3 * i as f32 / sr as f32).sin() * amp).collect();
        Shared::new(&gc_handle(), mudpie_bank(share(SampleData::from_channels(&s, &s, sr as u32, 41))))
    }

    #[test]
    fn the_bass_trim_lands_after_the_last_stages() {
        let settings = LaneSettings { knobs: Knobs::CENTERED, sound_seed: 3, pattern_seed: 3, form: None, rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: true, squash: 1.0 };
        let gain = bass_trim(|| PatternLane::bass(3, 3), pie(0.6), mudpie_params(), &settings, 48_000, 16);
        let post = render_lufs(PatternLane::bass(3, 3), pie(0.6), LaneSamplerParams { gain, ..mudpie_params() }, &settings, None, 48_000, 16, true);
        assert!((post - target_lufs(Role::Bass)).abs() < 0.5, "{post}");
    }

    #[test]
    fn trimmed_lanes_land_on_the_target() {
        let settings = LaneSettings { knobs: Knobs::CENTERED, sound_seed: 3, pattern_seed: 3, form: None, rate: 0, mode: 0, style: None, contrast: 0.5, unison: Default::default(), clips: false, squash: 0.0 };
        for amp in [0.05, 0.6] {
            let lufs = lane_lufs(PatternLane::bass(3, 3), pie(amp), mudpie_params(), &settings, None, 48_000, 16);
            assert!(lufs.is_finite());
            let gain = trim_gain(Role::Bass, lufs);
            let after = lufs + 20.0 * (gain as f64).log10();
            assert!((after - target_lufs(Role::Bass)).abs() < 0.01, "{amp}: {lufs} -> {after}");
        }
    }
}
