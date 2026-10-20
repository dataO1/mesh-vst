//! The plugin's parameters (design D37): eight automatable meta knobs, and
//! settings the DAW saves but does not automate.

use nice_plug::prelude::*;

/// What the instance plays (D1). Picked when the instance is set up.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Drums,
    Bass,
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

/// The drum style (D39): a rhythm family from Mesh live, or the pattern
/// seed's pick.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumStyle {
    Seed,
    #[name = "Half time"]
    HalfTime,
    #[name = "Two step"]
    TwoStep,
    Rolling,
    Jungle,
    Hardcore,
}

impl DrumStyle {
    /// Index into `macros::DRUM_STYLES`, `None` for the seed's pick.
    pub fn index(self) -> Option<u8> {
        match self {
            DrumStyle::Seed => None,
            DrumStyle::HalfTime => Some(0),
            DrumStyle::TwoStep => Some(1),
            DrumStyle::Rolling => Some(2),
            DrumStyle::Jungle => Some(3),
            DrumStyle::Hardcore => Some(4),
        }
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
/// Highest seed value.
pub const MAX_SEED: i32 = 9999;

#[derive(Params)]
pub struct MeshParams {
    #[id = "density"]
    pub density: FloatParam,
    #[id = "permute"]
    pub permute: FloatParam,
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

    #[id = "style"]
    pub style: EnumParam<DrumStyle>,
    #[id = "mode"]
    pub mode: EnumParam<Mode>,
    #[id = "sound_seed"]
    pub sound_seed: IntParam,
    #[id = "pattern_seed"]
    pub pattern_seed: IntParam,
    #[id = "form"]
    pub form: IntParam,
    #[id = "phase"]
    pub phase: EnumParam<PhaseMode>,
    #[id = "scale"]
    pub scale: EnumParam<Scale>,
    #[id = "rate"]
    pub rate: EnumParam<Rate>,
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
    #[id = "unison_mode"]
    pub unison_mode: EnumParam<UnisonSpread>,
    #[id = "gain"]
    pub gain: FloatParam,

    /// The samples the project plays (D18).
    #[persist = "kit"]
    pub kit: std::sync::Arc<std::sync::Mutex<crate::worker::KitState>>,
    /// Seed locks (D20).
    #[persist = "ui"]
    pub ui: std::sync::Arc<std::sync::Mutex<crate::ui::UiState>>,
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

fn setting(p: IntParam) -> IntParam {
    p.non_automatable()
}

impl Default for MeshParams {
    fn default() -> Self {
        let fresh = Fresh::new();
        Self {
            density: knob_at("Density", fresh.knob()),
            permute: knob_at("Permute", 0.0),
            variation: knob_at("Variation", fresh.knob()),
            swing: knob_at("Groove", fresh.knob()),
            shape: knob_at("Shape", fresh.knob()),
            morph: knob_at("Morph", fresh.knob()),
            spread: knob_at("Spread", fresh.knob()),
            slide: knob_at("Slide", fresh.knob()),
            tone: knob_at("Tone", fresh.knob()),
            // MUSICAL: a new instance starts warm, below the crush.
            heat: knob_at("Heat", 0.6 * (fresh.knob() - 0.25) + 0.1),
            style: EnumParam::new("Style", DrumStyle::Seed),
            mode: EnumParam::new("Mode", Mode::Drums).non_automatable(),
            sound_seed: setting(IntParam::new("Sound seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED })),
            pattern_seed: setting(IntParam::new("Pattern seed", fresh.seed(), IntRange::Linear { min: 0, max: MAX_SEED })),
            form: setting(IntParam::new("Form", 1, IntRange::Linear { min: 0, max: 10 }))
                .with_value_to_string(std::sync::Arc::new(|v| if v == 0 { "none".into() } else { v.to_string() }))
                .with_string_to_value(std::sync::Arc::new(|s| match s.trim() { "none" => Some(0), s => s.parse().ok() })),
            phase: EnumParam::new("Phase", PhaseMode::Continuous).non_automatable(),
            scale: EnumParam::new("Scale", Scale::Minor).non_automatable(),
            rate: EnumParam::new("Rate", Rate::Normal),
            // 0 = the seed picks; 1.. = BEND_RANGES.
            bend_range: setting(IntParam::new("Bend range", 0, IntRange::Linear { min: 0, max: BEND_RANGES.len() as i32 }))
                .with_value_to_string(std::sync::Arc::new(|v| if v <= 0 { "seed".into() } else { format!("{} st", BEND_RANGES[(v as usize - 1).min(6)]) }))
                .with_string_to_value(std::sync::Arc::new(|s| {
                    if s.trim() == "seed" {
                        return Some(0);
                    }
                    let st: i32 = s.trim().trim_end_matches("st").trim().parse().ok()?;
                    BEND_RANGES.iter().position(|&r| r == st).map(|i| i as i32 + 1)
                })),
            phrase: EnumParam::new("Phrase", Phrase::Eight).non_automatable(),
            new_root: EnumParam::new("New root", NewRootMode::Glide).non_automatable(),
            unison_voices: setting(IntParam::new("Unison voices", 4, IntRange::Linear { min: 1, max: 8 })),
            unison_detune: FloatParam::new("Unison detune", 25.0, FloatRange::Linear { min: 0.0, max: 60.0 })
                .with_unit(" ct")
                .with_step_size(0.5)
                .non_automatable(),
            unison_mode: EnumParam::new("Unison mode", UnisonSpread::Linear).non_automatable(),
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed { min: util::db_to_gain(-30.0), max: util::db_to_gain(6.0), factor: FloatRange::gain_skew_factor(-30.0, 6.0) },
            )
            .with_smoother(SmoothingStyle::Logarithmic(30.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            kit: Default::default(),
            ui: Default::default(),
        }
    }
}
