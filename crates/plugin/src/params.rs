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
/// Highest seed value.
pub const MAX_SEED: i32 = 9999;

#[derive(Params)]
pub struct MeshParams {
    #[id = "density"]
    pub density: FloatParam,
    #[id = "intensity"]
    pub intensity: FloatParam,
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
    #[id = "gain"]
    pub gain: FloatParam,

    /// The samples the project plays (D18).
    #[persist = "kit"]
    pub kit: std::sync::Arc<std::sync::Mutex<crate::worker::KitState>>,
}

fn knob(name: &str) -> FloatParam {
    FloatParam::new(name, 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
        .with_unit(" %")
}

fn setting(p: IntParam) -> IntParam {
    p.non_automatable()
}

impl Default for MeshParams {
    fn default() -> Self {
        Self {
            density: knob("Density"),
            intensity: knob("Intensity"),
            variation: knob("Variation"),
            swing: knob("Swing"),
            shape: knob("Shape"),
            morph: knob("Morph"),
            spread: knob("Spread"),
            slide: knob("Slide"),
            style: EnumParam::new("Style", DrumStyle::Seed),
            mode: EnumParam::new("Mode", Mode::Drums).non_automatable(),
            sound_seed: setting(IntParam::new("Sound seed", 1, IntRange::Linear { min: 0, max: MAX_SEED })),
            pattern_seed: setting(IntParam::new("Pattern seed", 1, IntRange::Linear { min: 0, max: MAX_SEED })),
            form: setting(IntParam::new("Form", 1, IntRange::Linear { min: 0, max: 10 }))
                .with_value_to_string(std::sync::Arc::new(|v| if v == 0 { "none".into() } else { v.to_string() }))
                .with_string_to_value(std::sync::Arc::new(|s| match s.trim() { "none" => Some(0), s => s.parse().ok() })),
            phase: EnumParam::new("Phase", PhaseMode::Continuous).non_automatable(),
            scale: EnumParam::new("Scale", Scale::Minor).non_automatable(),
            rate: EnumParam::new("Rate", Rate::Normal),
            bend_range: setting(IntParam::new("Bend range", 4, IntRange::Linear { min: 0, max: BEND_RANGES.len() as i32 - 1 }))
                .with_value_to_string(std::sync::Arc::new(|v| format!("{} st", BEND_RANGES[v.clamp(0, 6) as usize])))
                .with_string_to_value(std::sync::Arc::new(|s| {
                    let st: i32 = s.trim().trim_end_matches("st").trim().parse().ok()?;
                    BEND_RANGES.iter().position(|&r| r == st).map(|i| i as i32)
                })),
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
        }
    }
}
