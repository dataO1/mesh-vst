//! Data types the generators take from outside: curated level maps (idioms)
//! and the drum sub-voice kinds. In Mesh these came from `style.yaml`.

/// Curated generator data replacing the built-in tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdiomSpec {
    /// Grids-style per-step importance maps.
    pub level_maps: Vec<LevelMapSpec>,
}

/// One per-step importance map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelMapSpec {
    pub name: String,
    /// Archetype this map belongs to (drum lane).
    pub archetype: Option<String>,
    /// Voice or sub-voice the map drives (e.g. "kick", "hats").
    pub voice: Option<String>,
    /// One level (0..=255) per step, a multiple of 16 steps long.
    pub levels: Vec<u8>,
}

/// A drum sub-voice: a group of event targets that shares one sample pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubVoiceKind {
    /// Kick, snare, ghost.
    Backbone,
    /// Break loops, sliced.
    Break,
    /// Closed and open hats.
    Hats,
    /// Percussion hits.
    Perc,
    /// Rides and crashes.
    Ride,
    /// Shakers and tambourines.
    Shaker,
    /// Top loops, chopped in 16ths.
    Top,
}
