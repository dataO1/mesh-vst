//! The plugin's parameters (design D37): the meta knobs, the seeds and the
//! settings, all automatable (user, 2026-10-21: everything that can be).


use nice_plug::prelude::*;

/// What the instance plays (D1). Picked when the instance is set up.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Drums,
    Bass,
}

/// Where the bass line comes from (D50): the shipped and the user's clips,
/// or the generator.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BassSource {
    Clips,
    Generated,
}

/// Who plays the form (D60): the lane's own, or the DAW's notes (a key per
/// pattern, the velocity its operator).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormSource {
    Internal,
    External,
}

/// Where pattern bar 0 is (D8).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseMode {
    Continuous,
    Retrigger,
}

/// Scale of the root the held note gives (D11).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Minor,
    Dorian,
    Phrygian,
    #[name = "Harmonic minor"]
    HarmonicMinor,
    Major,
}

impl Scale {
    /// `(minor family, mode index)` as the harmony tables number them.
    pub fn family_mode(self) -> (bool, u8) {
        match self {
            Scale::Minor => (true, 0),
            Scale::Dorian => (true, 1),
            Scale::Phrygian => (true, 2),
            Scale::HarmonicMinor => (true, 3),
            Scale::Major => (false, 0),
        }
    }
}

/// The drum style (D39, D68): a rhythm family, or the pattern seed's pick.
/// New styles go at the end (a saved project stores the index).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumStyle {
    Seed,
    #[name = "DnB · Half time"]
    HalfTime,
    #[name = "DnB · Two step"]
    TwoStep,
    #[name = "DnB · Rolling"]
    Rolling,
    #[name = "DnB · Jungle"]
    Jungle,
    #[name = "Breaks · Hardcore"]
    Hardcore,
    #[name = "DnB · Techstep"]
    Techstep,
    #[name = "DnB · Neuro"]
    Neuro,
    #[name = "DnB · Jump up"]
    JumpUp,
    #[name = "DnB · Minimal"]
    Minimal,
    #[name = "Breaks · Rave amen"]
    RaveAmen,
    #[name = "Breaks · Nu skool"]
    NuSkool,
    #[name = "Breaks · Broken beat"]
    BrokenBeat,
    #[name = "Breaks · Electro"]
    Electro,
    #[name = "Bass · Dubstep"]
    Dubstep,
    #[name = "Bass · Grime"]
    Grime,
    #[name = "Bass · UK funky"]
    UkFunky,
    #[name = "Bass · Gqom"]
    Gqom,
    #[name = "Bass · Jersey club"]
    JerseyClub,
    #[name = "Bass · Footwork"]
    Footwork,
    #[name = "Bass · Dembow"]
    Dembow,
    #[name = "IDM · IDM"]
    Idm,
    #[name = "IDM · Drill'n'bass"]
    DrillNBass,
    #[name = "IDM · Breakcore"]
    Breakcore,
}

impl DrumStyle {
    /// Index into `macros::DRUM_STYLES`, `None` for the seed's pick.
    pub fn index(self) -> Option<u8> {
        (self as u8).checked_sub(1)
    }
}

/// Speed of the bass line on top of its seeded block speeds.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    Half,
    Normal,
    Double,
}

impl Rate {
    pub fn exponent(self) -> i8 {
        match self {
            Rate::Half => -1,
            Rate::Normal => 0,
            Rate::Double => 1,
        }
    }
}

/// How the drums grow their release tail above Shape's middle.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    #[name = "VOC"]
    Vocoder,
    #[name = "SPEC"]
    Spectral,
    #[name = "NOISE"]
    Noise,
}

impl Tail {
    pub fn mode(self) -> mesh_vst_core::tail::TailMode {
        use mesh_vst_core::tail::TailMode;
        match self {
            Tail::Vocoder => TailMode::Vocoder,
            Tail::Spectral => TailMode::Spectral,
            Tail::Noise => TailMode::Noise,
        }
    }

    /// The sound seed's tail.
    pub fn seeded(sound_seed: u32) -> Self {
        use mesh_vst_core::tail::TailMode;
        match TailMode::seeded(sound_seed) {
            TailMode::Vocoder => Tail::Vocoder,
            TailMode::Spectral => Tail::Spectral,
            TailMode::Noise => Tail::Noise,
        }
    }
}

/// Pitch bend ranges in semitones (D24).
pub const BEND_RANGES: [i32; 7] = [1, 3, 5, 7, 12, 19, 24];
/// Ranges the seed picks from for its default (D24), per mode.
pub const SEEDED_BEND_DRUMS: [i32; 3] = [12, 19, 24];
pub const SEEDED_BEND_BASS: [i32; 5] = [1, 3, 5, 7, 12];

/// Bars of a phrase (D25).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phrase {
    #[name = "4 bars"]
    Four,
    #[name = "8 bars"]
    Eight,
    #[name = "16 bars"]
    Sixteen,
}

impl Phrase {
    pub fn bars(self) -> u32 {
        match self {
            Phrase::Four => 4,
            Phrase::Eight => 8,
            Phrase::Sixteen => 16,
        }
    }
}

/// What a new held note does (D10).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewRootMode {
    Glide,
    Retrigger,
}

/// How unison copies spread (D38).
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnisonSpread {
    Linear,
    Soft,
    Hard,
    Stacked,
    Random,
}
/// Widest transpose either way, in semitones (two octaves).
pub const TRANSPOSE_MAX: i32 = 24;

/// Highest seed value.
pub const MAX_SEED: i32 = 9999;

#[derive(Params)]
pub struct MeshParams {
    #[id = "density"]
    pub density: FloatParam,
    #[id = "permute"]
    pub permute: FloatParam,
    /// D49: how far B moves from A, and C and D from them.
    #[id = "contrast"]
    pub contrast: FloatParam,
    #[id = "variation"]
    pub variation: FloatParam,
    #[id = "swing"]
    pub swing: FloatParam,
    #[id = "shape"]
    pub shape: FloatParam,
    #[id = "morph"]
    pub morph: FloatParam,
    #[id = "spread"]
    pub spread: FloatParam,
    #[id = "slide"]
    pub slide: FloatParam,

    #[id = "tone"]
    pub tone: FloatParam,

    #[id = "heat"]
    pub heat: FloatParam,

    /// The bass's neuro processing chain (D53): none .. a lot.
    #[id = "neuro"]
    pub neuro: FloatParam,

    #[id = "tail"]
    pub tail: EnumParam<Tail>,

    /// The last stage's multiband squash: mild .. strong.
    #[id = "squash"]
    pub squash: FloatParam,

    /// Splice (D42): 0 stacked on the host .. 0.5 off .. 1 in its gaps.
    #[id = "splice"]
    pub splice: FloatParam,

    #[id = "style"]
    pub style: EnumParam<DrumStyle>,
    #[id = "mode"]
    pub mode: EnumParam<Mode>,
    #[id = "sound_seed"]
    pub sound_seed: IntParam,
    #[id = "pattern_seed"]
    pub pattern_seed: IntParam,
    /// The generated form's seeds (D48): the four phrases, the fill plan,
    /// the second pass.
    #[id = "phrase_seed"]
    pub phrase_seed: IntParam,
    #[id = "fill_seed"]
    pub fill_seed: IntParam,
    #[id = "second_seed"]
    pub second_seed: IntParam,
    #[id = "phase"]
    pub phase: EnumParam<PhaseMode>,
    #[id = "scale"]
    pub scale: EnumParam<Scale>,
    #[id = "rate"]
    pub rate: EnumParam<Rate>,
    #[id = "bass_source"]
    pub bass_source: EnumParam<BassSource>,
    #[id = "bend_range"]
    pub bend_range: IntParam,
    #[id = "phrase"]
    pub phrase: EnumParam<Phrase>,
    #[id = "new_root"]
    pub new_root: EnumParam<NewRootMode>,
    #[id = "unison_voices"]
    pub unison_voices: IntParam,
    #[id = "unison_detune"]
    pub unison_detune: FloatParam,
    /// Stereo width of the unison copies (the centre each note varies around).
    #[id = "unison_width"]
    pub unison_width: FloatParam,
    /// The output below 80 Hz in mono (a global setting, not seeded).
    #[id = "mono_lows"]
    pub mono_lows: BoolParam,
    #[id = "unison_mode"]
    pub unison_mode: EnumParam<UnisonSpread>,
    #[id = "gain"]
    pub gain: FloatParam,
    /// D60: the lane's own form or the DAW's notes.
    #[id = "form_source"]
    pub form_source: EnumParam<FormSource>,
    /// D60: semitones on the root.
    #[id = "transpose"]
    pub transpose: IntParam,

    /// The samples the project plays (D18).
    #[persist = "kit"]
    pub kit: std::sync::Arc<std::sync::Mutex<crate::worker::KitState>>,
    /// Seed locks (D20).
    #[persist = "ui"]
    pub ui: std::sync::Arc<std::sync::Mutex<crate::ui::UiState>>,
    /// The patterns the user edited (D60).
    #[persist = "patterns"]
    pub patterns: std::sync::Arc<std::sync::Mutex<crate::patterns::PatternBooks>>,
}

/// Set to anything: a new instance starts at fixed defaults (seed 1, knobs
/// centred), for tests.
pub const FIXED_DEFAULTS_ENV: &str = "MESH_VST_FIXED_DEFAULTS";

/// A new instance's starting values: random seeds and knobs, so every new
/// instance plays something else. A saved state replaces them.
struct Fresh {
    state: Option<std::cell::Cell<u64>>,
}

impl Fresh {
    fn new() -> Self {
        if std::env::var_os(FIXED_DEFAULTS_ENV).is_some() {
            return Self { state: None };
        }
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Self { state: Some(std::cell::Cell::new(t ^ (&t as *const u64 as u64) | 1)) }
    }

    fn next(&self) -> Option<u64> {
        let s = self.state.as_ref()?;
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        Some(x)
    }

    fn seed(&self) -> i32 {
        self.next().map_or(1, |x| (x % (MAX_SEED as u64 + 1)) as i32)
    }

    // MUSICAL: away from the extremes.
    fn knob(&self) -> f32 {
        self.next().map_or(0.5, |x| 0.25 + 0.5 * (x % 1000) as f32 / 1000.0)
    }
}

fn knob_at(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
        .with_unit(" %")
}

impl Default for MeshParams {
    fn default() -> Self {
        let fresh = Fresh::new();
        let sound = fresh.seed();
        Self {
            density: knob_at("Density", fresh.knob()),
            permute: knob_at("Permute", 0.0),
            contrast: knob_at("Contrast", mesh_vst_core::contrast::CONTRAST_DEFAULT),
            variation: knob_at("Variation", fresh.knob()),
            swing: knob_at("Groove", fresh.knob()),
            shape: knob_at("Shape", fresh.knob()),
            // User, 2026-10-27: Morph starts off.
            morph: knob_at("Morph", 0.0),
            spread: knob_at("Spread", fresh.knob()),
            slide: knob_at("Slide", fresh.knob()),
            tone: knob_at("Tone", fresh.knob()),
            // MUSICAL: a new instance starts warm, below the crush.
            heat: knob_at("Heat", 0.6 * (fresh.knob() - 0.25) + 0.1),
            neuro: knob_at("Neuro", 0.0),
            tail: EnumParam::new("Tail", Tail::seeded(sound as u32)),
            splice: knob_at("Splice", 0.5),
            // The middle plays the sound seed's Squash (D66).
            squash: knob_at("Squash", 0.5),
            style: EnumParam::new("Style", DrumStyle::Seed),
            mode: EnumParam::new("Mode", Mode::Drums),
            sound_seed: IntParam::new("Sound seed", sound, IntRange::Linear { min: 0, max: MAX_SEED }),
            pattern_seed: IntParam::new("Pattern seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED }),
            phrase_seed: IntParam::new("Phrase seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED }),
            fill_seed: IntParam::new("Fill seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED }),
            second_seed: IntParam::new("Second pass seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED }),
            phase: EnumParam::new("Phase", PhaseMode::Continuous),
            scale: EnumParam::new("Scale", Scale::Minor),
            rate: EnumParam::new("Rate", Rate::Normal),
            bass_source: EnumParam::new("Bass source", BassSource::Clips),
            // 0 = the seed picks; 1.. = BEND_RANGES.
            bend_range: IntParam::new("Bend range", 0, IntRange::Linear { min: 0, max: BEND_RANGES.len() as i32 })
                .with_value_to_string(std::sync::Arc::new(|v| if v <= 0 { "seed".into() } else { format!("{} st", BEND_RANGES[(v as usize - 1).min(6)]) }))
                .with_string_to_value(std::sync::Arc::new(|s| {
                    if s.trim() == "seed" {
                        return Some(0);
                    }
                    let st: i32 = s.trim().trim_end_matches("st").trim().parse().ok()?;
                    BEND_RANGES.iter().position(|&r| r == st).map(|i| i as i32 + 1)
                })),
            phrase: EnumParam::new("Phrase", Phrase::Eight),
            new_root: EnumParam::new("New root", NewRootMode::Glide),
            unison_voices: IntParam::new("Unison voices", 4, IntRange::Linear { min: 1, max: 8 }),
            unison_detune: FloatParam::new("Unison detune", 25.0, FloatRange::Linear { min: 0.0, max: 60.0 })
                .with_unit(" ct")
                .with_step_size(0.5),
            unison_width: FloatParam::new("Unison width", 0.6, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage())
                .with_unit(" %"),
            mono_lows: BoolParam::new("Mono lows", true),
            unison_mode: EnumParam::new("Unison mode", UnisonSpread::Linear),
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed { min: util::db_to_gain(-30.0), max: util::db_to_gain(6.0), factor: FloatRange::gain_skew_factor(-30.0, 6.0) },
            )
            .with_smoother(SmoothingStyle::Logarithmic(30.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            form_source: EnumParam::new("Form source", FormSource::Internal),
            transpose: IntParam::new("Transpose", 0, IntRange::Linear { min: -TRANSPOSE_MAX, max: TRANSPOSE_MAX }).with_unit(" st"),
            kit: Default::default(),
            ui: Default::default(),
            patterns: Default::default(),
        }
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn every_style_names_its_generator_style() {
        let names = DrumStyle::variants();
        assert_eq!(names.len(), mesh_vst_core::macros::DRUM_STYLES.len() + 1);
        for (i, style) in mesh_vst_core::macros::DRUM_STYLES.iter().enumerate() {
            let label = names[i + 1].rsplit("· ").next().unwrap_or("").to_lowercase().replace([' ', '\''], "_");
            assert_eq!(&label, style, "variant {}", i + 1);
            assert_eq!(DrumStyle::from_index(i + 1).index(), Some(i as u8));
        }
        assert_eq!(DrumStyle::Seed.index(), None);
    }
}
