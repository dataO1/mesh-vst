//! The drum styles as seeded rules (D68): each style has a few written
//! kick and snare backbones (its anchors), the steps where a seed may add a
//! hit, its hat and ghost maps, the hat patterns it plays and its velocity
//! motives. A letter's seed draws one backbone, its extra hits and its
//! motives, so every roll of a style plays the style differently. The first
//! backbone of the first five styles is their table from before (D39).
//!
//! All styles stay in 4/4 and none plays a kick on every beat as its
//! anchors (the hardcore of D39 aside).

use super::blocks::LevelMap;
use super::drums::Archetype;
use crate::rng;
use crate::STEPS_PER_BAR;

/// A style's family: the B letter plays another style of A's (D68).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Dnb,
    Breaks,
    Bass,
    Idm,
}

/// Velocity offsets over a bar, for the backbone voices (kick, snare,
/// ghosts, percussion, the break) and the top voices (hats, ride, shaker,
/// the top loop).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motive {
    pub back: [i8; STEPS_PER_BAR],
    pub top: [i8; STEPS_PER_BAR],
}

/// One style. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleDef {
    pub name: &'static str,
    pub group: Group,
    /// Written backbones; a seed plays one.
    pub kicks: &'static [[u8; STEPS_PER_BAR]],
    pub snares: &'static [[u8; STEPS_PER_BAR]],
    /// Chance (of 255) a seed adds a kick or a snare on an empty step.
    pub maybe_kick: [u8; STEPS_PER_BAR],
    pub maybe_snare: [u8; STEPS_PER_BAR],
    pub hats: [u8; STEPS_PER_BAR],
    pub ghost: [u8; STEPS_PER_BAR],
    /// Hat patterns ([`super::recipe::HATS_PATTERNS`]) the style plays.
    pub hat_patterns: &'static [usize],
    /// Velocity motives ([`MOTIVES`]) the style plays.
    pub motives: &'static [usize],
}

// MUSICAL: tune by ear (user, 2026-10-27). An added hit's level range: it
// sits below the anchors, so Density brings it in.
const ADDED_LEVEL: (f32, f32) = (70.0, 175.0);

// RNG streams (private to the styles).
const S_KICK_PICK: u32 = 0x600;
const S_SNARE_PICK: u32 = 0x601;
const S_ADD_KICK: u32 = 0x602;
const S_ADD_SNARE: u32 = 0x603;
const S_ADD_LEVEL: u32 = 0x604;
const S_MOTIVE: u32 = 0x605;
const S_MOTIVE_SECOND: u32 = 0x606;

const fn steps(list: &[(usize, u8)]) -> [u8; STEPS_PER_BAR] {
    let mut out = [0u8; STEPS_PER_BAR];
    let mut i = 0;
    while i < list.len() {
        out[list[i].0] = list[i].1;
        i += 1;
    }
    out
}

// ─── Velocity motives ──────────────────────────────────────────────────

const Z: [i8; 16] = [0; 16];

// MUSICAL: first draft, tune by ear (user, 2026-10-27: velocities form a
// groove with a motive, never random; plenty per style).
pub const MOTIVES: [Motive; 20] = [
    // 0 Backbeat: the snares and the on-beat hats lead.
    Motive { back: [6, 0, 0, 0, 10, 0, 0, 0, 2, 0, 0, 0, 10, 0, 0, 0], top: [6, -8, 2, -8, 6, -8, 2, -8, 6, -8, 2, -8, 6, -8, 2, -8] },
    // 1 Offbeat lift: the off 8ths of the hats ring out.
    Motive { back: [8, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0], top: [-6, -10, 10, -10, -6, -10, 10, -10, -6, -10, 10, -10, -6, -10, 10, -10] },
    // 2 Rise to the 3: the hats swell through the first half.
    Motive { back: Z, top: [-10, -9, -7, -5, -3, -1, 2, 5, 10, -8, -6, -4, -2, 0, 2, 4] },
    // 3 Falling: each half starts strong and fades.
    Motive { back: [6, 0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0], top: [10, 8, 6, 4, 2, 0, -2, -4, 8, 6, 4, 2, 0, -2, -4, -6] },
    // 4 Pushed snare: the ghost before each backbeat leans in.
    Motive { back: [0, 0, -4, 8, 6, 0, 0, 0, 0, 0, -4, 8, 6, 0, 0, 0], top: [2, -4, 4, 0, 2, -4, 4, 0, 2, -4, 4, 0, 2, -4, 4, 0] },
    // 5 Ghost answer: soft ghosts, the last ones answer louder.
    Motive { back: [0, -6, 0, 4, 0, -6, 0, 8, 0, -6, 0, 4, 0, -6, 6, 10], top: [4, -6, 2, -4, 4, -6, 2, -4, 4, -6, 2, -4, 4, -6, 2, -4] },
    // 6 Call and response: the first half strong, the second answers softer.
    Motive { back: [6, 4, 4, 4, 6, 4, 4, 4, -2, -4, -4, -4, 2, -4, -4, -4], top: [6, 2, 4, 2, 6, 2, 4, 2, -4, -6, -2, -6, -4, -6, -2, -6] },
    // 7 Lift into the next bar.
    Motive { back: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 6, 10, 14], top: [0, -2, 0, -2, 0, -2, 0, -2, 2, 2, 4, 4, 6, 8, 10, 12] },
    // 8 Heavy downbeat.
    Motive { back: [14, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0], top: [10, -6, -2, -6, 0, -6, -2, -6, 6, -6, -2, -6, 0, -6, -2, -6] },
    // 9 Gallop: the 16th pickups into each off 8th.
    Motive { back: Z, top: [6, -10, 4, 8, 6, -10, 4, 8, 6, -10, 4, 8, 6, -10, 4, 8] },
    // 10 Threes across the bar (every third 16th leads).
    Motive { back: [8, 0, 0, 6, 0, 0, 6, 0, 0, 6, 0, 0, 6, 0, 0, 6], top: [10, -6, -6, 10, -6, -6, 10, -6, -6, 10, -6, -6, 10, -6, -6, 10] },
    // 11 Fives across the bar.
    Motive { back: [8, 0, 0, 0, 0, 6, 0, 0, 0, 0, 6, 0, 0, 0, 0, 6], top: [12, -4, -4, -4, -4, 12, -4, -4, -4, -4, 12, -4, -4, -4, -4, 12] },
    // 12 Swung 16ths: the off 16ths whisper.
    Motive { back: [4, -6, 0, -6, 4, -6, 0, -6, 4, -6, 0, -6, 4, -6, 0, -6], top: [4, -12, 0, -8, 4, -12, 0, -8, 4, -12, 0, -8, 4, -12, 0, -8] },
    // 13 Half bars: the 1 and the 3 lead.
    Motive { back: [8, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0], top: [8, -4, -4, -4, -4, -4, -4, -4, 8, -4, -4, -4, -4, -4, -4, -4] },
    // 14 Shuffle: the last 16th of each beat leads.
    Motive { back: [2, 0, 0, 6, 2, 0, 0, 6, 2, 0, 0, 6, 2, 0, 0, 6], top: [6, -8, -2, 8, 6, -8, -2, 8, 6, -8, -2, 8, 6, -8, -2, 8] },
    // 15 Even: as written.
    Motive { back: Z, top: Z },
    // 16 Roll into the 4.
    Motive { back: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -4, -2, 2, 6, 10, 12], top: [2, -2, 0, -2, 2, -2, 0, -2, 0, 2, 4, 6, 8, 10, 12, 14] },
    // 17 Anticipation: the 16th before each beat.
    Motive { back: [4, 0, 0, 8, 4, 0, 0, 8, 4, 0, 0, 8, 4, 0, 0, 8], top: [2, -4, -2, 10, 2, -4, -2, 10, 2, -4, -2, 10, 2, -4, -2, 10] },
    // 18 Two and four: the backbeat alone stands out.
    Motive { back: [-2, -4, -4, -4, 12, -4, -4, -4, -2, -4, -4, -4, 12, -4, -4, -4], top: [-4, -6, 0, -6, 6, -6, 0, -6, -4, -6, 0, -6, 6, -6, 0, -6] },
    // 19 Accent drift: the loud hits wander through the bar.
    Motive { back: [8, 0, 0, 0, 0, 6, 0, 0, 0, 0, 4, 0, 0, 0, 0, 10], top: [8, -4, 0, -4, -2, 8, -4, 0, -4, -2, 8, -4, 0, -4, -2, 8] },
];

/// The motives a fill bar draws from.
const FILL_MOTIVES: [usize; 4] = [7, 16, 2, 6];

// ─── Styles ────────────────────────────────────────────────────────────

use super::drums::{
    GHOST_HALF, GHOST_HARDCORE, GHOST_JUNGLE, GHOST_ROLLING, GHOST_TWO_STEP, HALF_TIME_KICK, HALF_TIME_SNARE, HARDCORE_KICK,
    HARDCORE_SNARE, HATS_8THS, HATS_HALF, HATS_OFFBEAT, HATS_RIDE, HATS_SPARSE, JUNGLE_KICK, JUNGLE_SNARE, ROLLING_KICK,
    ROLLING_SNARE, TWO_STEP_KICK, TWO_STEP_SNARE,
};

/// No extra hits.
const NONE: [u8; 16] = [0; 16];
/// Every hat pattern of D39.
const HATS_ALL: &[usize] = &[0, 1, 2, 3, 4];
const HATS_16: [u8; 16] = [180, 100, 230, 110, 180, 100, 230, 110, 180, 100, 230, 110, 180, 100, 230, 120];
const HATS_SHUFFLE: [u8; 16] = [120, 60, 180, 220, 120, 60, 180, 220, 120, 60, 180, 220, 120, 60, 180, 230];
const GHOST_SPARSE: [u8; 16] = [0, 30, 0, 80, 0, 40, 0, 120, 0, 30, 0, 80, 0, 50, 90, 130];
const GHOST_BUSY: [u8; 16] = [0, 140, 120, 180, 0, 150, 170, 210, 120, 200, 150, 180, 0, 160, 190, 220];

// MUSICAL: first draft, tune by ear (user, 2026-10-27: more styles, also
// niche ones that break the beat; picked by the seed or the Style setting).
// Steps 0, 4, 8, 12 are the beats. Levels from 230 are anchors, from 128
// they play at the middle Density.
pub const STYLES: [StyleDef; 23] = [
    // ── The five of D39, their first backbone as before.
    StyleDef {
        name: "half_time",
        group: Group::Dnb,
        kicks: &[HALF_TIME_KICK, [255, 0, 0, 90, 0, 0, 0, 0, 0, 0, 110, 0, 0, 60, 0, 0], [255, 0, 0, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0, 120, 80]],
        snares: &[HALF_TIME_SNARE, steps(&[(8, 255), (14, 60), (15, 50)])],
        maybe_kick: steps(&[(3, 50), (6, 60), (11, 40), (13, 50)]),
        maybe_snare: steps(&[(11, 30), (15, 40)]),
        hats: HATS_HALF,
        ghost: GHOST_HALF,
        hat_patterns: HATS_ALL,
        motives: &[0, 3, 5, 8, 13, 15, 16, 18],
    },
    StyleDef {
        name: "two_step",
        group: Group::Dnb,
        kicks: &[TWO_STEP_KICK, [255, 0, 0, 0, 0, 0, 0, 90, 0, 0, 250, 0, 0, 0, 80, 0], [255, 0, 70, 0, 0, 0, 0, 0, 0, 0, 250, 0, 0, 60, 0, 0]],
        snares: &[TWO_STEP_SNARE, steps(&[(4, 255), (12, 255), (14, 80)]), steps(&[(4, 255), (9, 60), (12, 255)])],
        maybe_kick: steps(&[(2, 40), (6, 50), (7, 60), (13, 50), (14, 40)]),
        maybe_snare: steps(&[(7, 40), (9, 40), (15, 50)]),
        hats: HATS_8THS,
        ghost: GHOST_TWO_STEP,
        hat_patterns: HATS_ALL,
        motives: &[0, 1, 4, 5, 6, 9, 15, 17, 18],
    },
    StyleDef {
        name: "rolling",
        group: Group::Dnb,
        kicks: &[ROLLING_KICK, [255, 0, 0, 0, 0, 0, 0, 150, 0, 0, 250, 0, 0, 90, 0, 0], [255, 0, 0, 0, 0, 0, 0, 150, 0, 0, 250, 0, 0, 0, 100, 70]],
        snares: &[ROLLING_SNARE, steps(&[(4, 255), (12, 255), (14, 70), (15, 90)])],
        maybe_kick: steps(&[(2, 40), (5, 30), (13, 50)]),
        maybe_snare: steps(&[(7, 40), (9, 40), (11, 30)]),
        hats: HATS_RIDE,
        ghost: GHOST_ROLLING,
        hat_patterns: HATS_ALL,
        motives: &[0, 2, 4, 5, 9, 12, 14, 17],
    },
    StyleDef {
        name: "jungle",
        group: Group::Dnb,
        kicks: &[JUNGLE_KICK, [255, 0, 200, 0, 0, 0, 0, 0, 0, 0, 230, 0, 0, 0, 120, 0], [255, 0, 200, 0, 0, 0, 90, 0, 0, 0, 230, 180, 0, 0, 0, 0]],
        snares: &[JUNGLE_SNARE, steps(&[(4, 255), (7, 150), (12, 240), (14, 110), (15, 140)])],
        maybe_kick: steps(&[(5, 40), (8, 30), (13, 40)]),
        maybe_snare: steps(&[(1, 30), (6, 40), (13, 40)]),
        hats: HATS_SPARSE,
        ghost: GHOST_JUNGLE,
        hat_patterns: HATS_ALL,
        motives: &[0, 4, 5, 6, 12, 14, 16, 19],
    },
    StyleDef {
        name: "hardcore",
        group: Group::Breaks,
        kicks: &[HARDCORE_KICK, [255, 0, 0, 0, 250, 0, 0, 80, 250, 0, 0, 0, 250, 0, 0, 0]],
        snares: &[HARDCORE_SNARE, steps(&[(4, 230), (12, 230), (15, 70)])],
        maybe_kick: steps(&[(2, 30), (10, 40), (14, 50)]),
        maybe_snare: steps(&[(7, 30), (13, 40)]),
        hats: HATS_OFFBEAT,
        ghost: GHOST_HARDCORE,
        hat_patterns: HATS_ALL,
        motives: &[1, 8, 13, 15, 16, 18],
    },
    // ── Drum and bass.
    StyleDef {
        name: "techstep",
        group: Group::Dnb,
        kicks: &[steps(&[(0, 255), (10, 250)]), steps(&[(0, 255), (7, 140), (10, 250)]), steps(&[(0, 255), (10, 250), (13, 120)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 255), (15, 80)])],
        maybe_kick: steps(&[(6, 40), (7, 50), (13, 60), (14, 40)]),
        maybe_snare: steps(&[(9, 30), (11, 30)]),
        hats: HATS_SPARSE,
        ghost: GHOST_SPARSE,
        hat_patterns: &[0, 1, 5, 8],
        motives: &[0, 3, 8, 12, 13, 18],
    },
    StyleDef {
        name: "neuro",
        group: Group::Dnb,
        kicks: &[steps(&[(0, 255), (10, 250), (13, 160)]), steps(&[(0, 255), (6, 180), (10, 250)]), steps(&[(0, 255), (3, 150), (10, 250), (11, 120)])],
        snares: &[steps(&[(4, 255), (12, 255), (15, 90)]), steps(&[(4, 255), (12, 255), (7, 70)])],
        maybe_kick: steps(&[(2, 40), (7, 50), (13, 60)]),
        maybe_snare: steps(&[(9, 40), (14, 40)]),
        hats: HATS_16,
        ghost: GHOST_ROLLING,
        hat_patterns: &[0, 3, 5, 6],
        motives: &[0, 4, 9, 12, 14, 17, 19],
    },
    StyleDef {
        name: "jump_up",
        group: Group::Dnb,
        kicks: &[steps(&[(0, 255), (10, 230), (13, 180)]), steps(&[(0, 255), (3, 150), (10, 230)]), steps(&[(0, 255), (6, 140), (10, 230), (11, 120)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 255), (14, 90)])],
        maybe_kick: steps(&[(2, 50), (7, 60), (14, 50)]),
        maybe_snare: steps(&[(7, 40), (15, 50)]),
        hats: HATS_8THS,
        ghost: GHOST_TWO_STEP,
        hat_patterns: &[0, 1, 3, 7],
        motives: &[0, 1, 7, 9, 17, 18],
    },
    StyleDef {
        name: "minimal",
        group: Group::Dnb,
        kicks: &[steps(&[(0, 255), (10, 240)]), steps(&[(0, 255), (11, 230)])],
        snares: &[steps(&[(4, 255), (12, 255)])],
        maybe_kick: steps(&[(7, 30), (14, 30)]),
        maybe_snare: steps(&[(15, 30)]),
        hats: HATS_SPARSE,
        ghost: GHOST_SPARSE,
        hat_patterns: &[1, 8, 0],
        motives: &[0, 13, 15, 18, 3],
    },
    // ── Breaks.
    StyleDef {
        name: "rave_amen",
        group: Group::Breaks,
        kicks: &[steps(&[(0, 255), (2, 220), (10, 230)]), steps(&[(0, 255), (2, 220), (10, 230), (11, 170)])],
        snares: &[steps(&[(4, 255), (7, 160), (12, 240), (15, 150)]), steps(&[(4, 255), (9, 160), (12, 240), (14, 140)])],
        maybe_kick: steps(&[(6, 40), (13, 50)]),
        maybe_snare: steps(&[(1, 40), (6, 50), (11, 40), (13, 40)]),
        hats: HATS_SPARSE,
        ghost: GHOST_JUNGLE,
        hat_patterns: &[0, 1, 2, 5],
        motives: &[0, 5, 7, 14, 16, 19],
    },
    StyleDef {
        name: "nu_skool",
        group: Group::Breaks,
        kicks: &[steps(&[(0, 255), (10, 240), (14, 140)]), steps(&[(0, 255), (6, 220), (10, 240)]), steps(&[(0, 255), (3, 140), (10, 240)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 255), (15, 80)])],
        maybe_kick: steps(&[(7, 50), (11, 40), (13, 40)]),
        maybe_snare: steps(&[(9, 40), (14, 30)]),
        hats: HATS_16,
        ghost: GHOST_TWO_STEP,
        hat_patterns: &[0, 1, 3, 5, 6],
        motives: &[0, 1, 4, 9, 12, 17],
    },
    StyleDef {
        name: "broken_beat",
        group: Group::Breaks,
        kicks: &[steps(&[(0, 255), (7, 230), (11, 200)]), steps(&[(0, 255), (6, 220), (14, 200)]), steps(&[(0, 255), (3, 180), (11, 220)])],
        snares: &[steps(&[(4, 255), (13, 240)]), steps(&[(4, 255), (12, 180), (13, 230)]), steps(&[(5, 220), (12, 255)])],
        maybe_kick: steps(&[(2, 40), (9, 50), (15, 40)]),
        maybe_snare: steps(&[(7, 40), (10, 40), (15, 50)]),
        hats: HATS_SHUFFLE,
        ghost: GHOST_BUSY,
        hat_patterns: &[1, 3, 6, 7],
        motives: &[4, 6, 14, 17, 19, 11],
    },
    StyleDef {
        name: "electro",
        group: Group::Breaks,
        kicks: &[steps(&[(0, 255), (6, 220), (10, 240)]), steps(&[(0, 255), (10, 240), (11, 190)]), steps(&[(0, 255), (3, 170), (10, 240)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 255), (15, 120)])],
        maybe_kick: steps(&[(7, 40), (13, 50), (14, 40)]),
        maybe_snare: steps(&[(14, 40), (7, 30)]),
        hats: HATS_16,
        ghost: GHOST_SPARSE,
        hat_patterns: &[0, 5, 6],
        motives: &[0, 2, 8, 13, 15, 18],
    },
    // ── Bass music.
    StyleDef {
        name: "dubstep",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255)]), steps(&[(0, 255), (3, 120), (14, 110)]), steps(&[(0, 255), (10, 120)])],
        snares: &[steps(&[(8, 255)]), steps(&[(8, 255), (15, 70)])],
        maybe_kick: steps(&[(3, 60), (6, 50), (10, 60), (11, 40), (14, 60)]),
        maybe_snare: steps(&[(11, 30), (14, 30)]),
        hats: HATS_HALF,
        ghost: GHOST_HALF,
        hat_patterns: &[0, 2, 4, 8],
        motives: &[3, 8, 10, 13, 16],
    },
    StyleDef {
        name: "grime",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255), (6, 230)]), steps(&[(0, 255), (3, 200), (10, 180)]), steps(&[(0, 255), (6, 230), (11, 160)])],
        snares: &[steps(&[(8, 255)]), steps(&[(4, 255), (12, 255)])],
        maybe_kick: steps(&[(9, 50), (13, 50), (14, 40)]),
        maybe_snare: steps(&[(15, 40)]),
        hats: HATS_8THS,
        ghost: GHOST_SPARSE,
        hat_patterns: &[0, 2, 4, 9],
        motives: &[0, 8, 10, 13, 17],
    },
    StyleDef {
        name: "uk_funky",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255), (7, 230), (10, 240)]), steps(&[(0, 255), (6, 220), (11, 220)]), steps(&[(0, 255), (3, 180), (8, 200), (14, 180)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 255), (14, 90)])],
        maybe_kick: steps(&[(2, 40), (13, 50), (15, 40)]),
        maybe_snare: steps(&[(7, 40), (10, 30)]),
        hats: HATS_SHUFFLE,
        ghost: GHOST_BUSY,
        hat_patterns: &[1, 3, 7, 9],
        motives: &[1, 9, 12, 14, 17],
    },
    StyleDef {
        name: "gqom",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255), (3, 240), (6, 240), (10, 230)]), steps(&[(0, 255), (3, 240), (6, 240), (12, 200)]), steps(&[(0, 255), (6, 240), (11, 230)])],
        snares: &[steps(&[(12, 255)]), steps(&[(4, 220), (12, 255)])],
        maybe_kick: steps(&[(9, 50), (13, 50), (14, 40)]),
        maybe_snare: steps(&[(8, 40), (15, 40)]),
        hats: HATS_OFFBEAT,
        ghost: GHOST_SPARSE,
        hat_patterns: &[1, 2, 7, 4],
        motives: &[8, 10, 13, 14, 16],
    },
    StyleDef {
        name: "jersey_club",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255), (6, 240), (10, 240), (12, 230)]), steps(&[(0, 255), (6, 240), (8, 220), (10, 240), (14, 160)])],
        snares: &[steps(&[(4, 255), (12, 255)]), steps(&[(4, 255), (12, 240), (13, 150)])],
        maybe_kick: steps(&[(3, 50), (14, 60), (15, 40)]),
        maybe_snare: steps(&[(7, 40), (15, 50)]),
        hats: HATS_8THS,
        ghost: GHOST_SPARSE,
        hat_patterns: &[0, 2, 4, 9],
        motives: &[0, 7, 10, 16, 18],
    },
    StyleDef {
        name: "footwork",
        group: Group::Bass,
        kicks: &[steps(&[(0, 255), (3, 240), (6, 240), (10, 240), (13, 230)]), steps(&[(0, 255), (3, 240), (8, 220), (11, 240)])],
        snares: &[steps(&[(4, 230), (12, 255)]), steps(&[(8, 255)])],
        maybe_kick: steps(&[(9, 50), (14, 50), (15, 40)]),
        maybe_snare: steps(&[(7, 40), (15, 40)]),
        hats: HATS_SPARSE,
        ghost: GHOST_SPARSE,
        hat_patterns: &[2, 4, 7],
        motives: &[10, 11, 14, 16, 19],
    },
    StyleDef {
        name: "dembow",
        group: Group::Bass,
        // The kick on the 1 and the 3 (the beats between it only with Density).
        kicks: &[steps(&[(0, 255), (4, 100), (8, 250), (12, 100)]), steps(&[(0, 255), (8, 250), (14, 120)])],
        snares: &[steps(&[(3, 240), (6, 240), (11, 240), (14, 240)]), steps(&[(3, 240), (7, 220), (11, 240), (14, 240)])],
        maybe_kick: steps(&[(10, 40), (15, 40)]),
        maybe_snare: NONE,
        hats: HATS_OFFBEAT,
        ghost: GHOST_SPARSE,
        hat_patterns: &[1, 9, 2],
        motives: &[14, 17, 13, 1],
    },
    // ── IDM.
    StyleDef {
        name: "idm",
        group: Group::Idm,
        kicks: &[steps(&[(0, 255), (5, 210), (10, 230), (15, 180)]), steps(&[(0, 255), (3, 200), (9, 230), (14, 170)]), steps(&[(0, 255), (7, 220), (11, 200)])],
        snares: &[steps(&[(6, 240), (12, 255)]), steps(&[(4, 255), (13, 230)]), steps(&[(7, 240), (12, 230), (15, 140)])],
        maybe_kick: steps(&[(2, 70), (8, 60), (12, 50), (13, 70)]),
        maybe_snare: steps(&[(1, 50), (9, 60), (10, 50), (14, 60)]),
        hats: HATS_SHUFFLE,
        ghost: GHOST_BUSY,
        hat_patterns: &[2, 4, 6, 7, 3],
        motives: &[10, 11, 19, 2, 3, 6, 12],
    },
    StyleDef {
        name: "drill_n_bass",
        group: Group::Idm,
        kicks: &[steps(&[(0, 255), (10, 240)]), steps(&[(0, 255), (2, 200), (10, 240), (11, 190)])],
        snares: &[steps(&[(4, 255), (12, 255), (7, 160), (15, 160)]), steps(&[(4, 255), (12, 255), (9, 150), (14, 170)])],
        maybe_kick: steps(&[(3, 60), (6, 60), (13, 70), (14, 50)]),
        maybe_snare: steps(&[(1, 60), (2, 50), (6, 70), (10, 60), (11, 60), (13, 70)]),
        hats: HATS_16,
        ghost: GHOST_BUSY,
        hat_patterns: &[5, 6, 3, 4],
        motives: &[5, 7, 12, 16, 19, 10],
    },
    StyleDef {
        name: "breakcore",
        group: Group::Idm,
        kicks: &[steps(&[(0, 255), (2, 230), (10, 240), (11, 230)]), steps(&[(0, 255), (3, 220), (8, 220), (10, 240)])],
        snares: &[steps(&[(4, 255), (7, 200), (9, 200), (12, 255), (14, 200), (15, 200)]), steps(&[(4, 255), (6, 190), (12, 255), (13, 200), (15, 210)])],
        maybe_kick: steps(&[(5, 70), (6, 60), (13, 70), (14, 60)]),
        maybe_snare: steps(&[(1, 70), (2, 60), (10, 70), (11, 60)]),
        hats: HATS_16,
        ghost: GHOST_BUSY,
        hat_patterns: &[5, 6, 4],
        motives: &[5, 7, 16, 19, 11, 10],
    },
];

/// The style's index of that name.
pub fn index_of(name: &str) -> Option<usize> {
    STYLES.iter().position(|s| s.name == name)
}

/// The styles of `index`'s group other than it.
pub fn related(index: usize) -> impl Iterator<Item = usize> {
    let group = STYLES[index.min(STYLES.len() - 1)].group;
    (0..STYLES.len()).filter(move |&i| i != index && STYLES[i].group == group)
}

/// The archetype of `def` as written (its first backbones).
pub fn archetype(def: &StyleDef) -> Archetype {
    Archetype {
        name: def.name.to_string(),
        buildup: false,
        kick: LevelMap::bar(def.kicks[0]),
        snare: LevelMap::bar(def.snares[0]),
        hats: LevelMap::bar(def.hats),
        ghost: LevelMap::bar(def.ghost),
        perc: None,
        style: index_of(def.name).map(|i| i as u8),
    }
}

/// A letter's kick and snare levels in style `def` (D68): one of its
/// backbones, and its extra hits, drawn from the letter `seed`.
pub fn seeded_backbone(def: &StyleDef, seed: u32, lane: u8) -> ([u8; STEPS_PER_BAR], [u8; STEPS_PER_BAR]) {
    let h = |stream: u32, n: u8| rng::hash(seed, lane, 0, n, stream);
    let mut kick = def.kicks[rng::below(h(S_KICK_PICK, 0), def.kicks.len() as u32) as usize];
    let mut snare = def.snares[rng::below(h(S_SNARE_PICK, 0), def.snares.len() as u32) as usize];
    for s in 0..STEPS_PER_BAR {
        let n = s as u8;
        if kick[s] == 0 && snare[s] == 0 && def.maybe_kick[s] > 0 && rng::chance(h(S_ADD_KICK, n), def.maybe_kick[s] as f32 / 255.0) {
            kick[s] = rng::range_f32(h(S_ADD_LEVEL, n), ADDED_LEVEL.0, ADDED_LEVEL.1).round() as u8;
        }
        if kick[s] == 0 && snare[s] == 0 && def.maybe_snare[s] > 0 && rng::chance(h(S_ADD_SNARE, n), def.maybe_snare[s] as f32 / 255.0) {
            snare[s] = rng::range_f32(h(S_ADD_LEVEL, n + 16), ADDED_LEVEL.0, ADDED_LEVEL.1).round() as u8;
        }
    }
    (kick, snare)
}

/// The velocity offsets a letter plays at `step` (D68): the motive its seed
/// draws, a fill bar its own; with Variation the second half of the bar
/// answers with a second motive.
pub fn motive_at(def: &StyleDef, seed: u32, lane: u8, fill: bool, variation: f32) -> (Motive, Motive) {
    let h = |stream: u32, n: u8| rng::hash(seed, lane, 0, n, stream);
    let pick = |pool: &[usize], n: u8| MOTIVES[pool[rng::below(h(S_MOTIVE, n), pool.len() as u32) as usize] % MOTIVES.len()];
    let first = pick(def.motives, 0);
    let second = if fill {
        pick(&FILL_MOTIVES, 1)
    } else if rng::chance(h(S_MOTIVE_SECOND, 0), variation.clamp(0.0, 1.0)) {
        pick(def.motives, 2)
    } else {
        first
    };
    (first, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_is_well_formed() {
        for (i, s) in STYLES.iter().enumerate() {
            assert_eq!(index_of(s.name), Some(i), "{}", s.name);
            assert!(!s.kicks.is_empty() && !s.snares.is_empty() && !s.motives.is_empty(), "{}", s.name);
            assert!(s.kicks.iter().all(|k| k[0] == 255), "{}: the 1 holds the kick", s.name);
            assert!(s.hat_patterns.iter().all(|&p| p < super::super::recipe::HATS_PATTERNS.len()), "{}", s.name);
            assert!(s.motives.iter().all(|&m| m < MOTIVES.len()), "{}", s.name);
            assert!(related(i).count() >= 2, "{}: B needs relatives", s.name);
        }
    }

    #[test]
    fn no_new_style_plays_four_on_the_floor() {
        for s in STYLES.iter().filter(|s| s.name != "hardcore") {
            for k in s.kicks {
                let beats = [0, 4, 8, 12].iter().filter(|&&b| k[b] >= 128).count();
                assert!(beats < 4, "{}: {k:?}", s.name);
            }
        }
    }

    #[test]
    fn seeds_roll_different_backbones_inside_the_style() {
        for s in &STYLES {
            let rolls: std::collections::BTreeSet<_> = (0..64).map(|seed| seeded_backbone(s, seed, 1)).collect();
            assert!(rolls.len() > 3, "{}: {} rolls", s.name, rolls.len());
            for (k, sn) in &rolls {
                // The anchors of one of the style's backbones stay.
                assert!(s.kicks.iter().any(|w| (0..16).all(|i| w[i] < 230 || k[i] == w[i])), "{}", s.name);
                assert!(s.snares.iter().any(|w| (0..16).all(|i| w[i] < 230 || sn[i] == w[i])), "{}", s.name);
            }
        }
    }
}
