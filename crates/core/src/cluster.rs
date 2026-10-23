//! Density's fills in clusters (user, 2026-10-23: the fills landed one by
//! one on random rests, a lone hit every two bars that made no rhythmic
//! sense). A bar's rests on the fill grid group into gaps between its
//! notes; Density picks whole gaps and fills a run in each: a lead-in that
//! ends on the next note, or a trail after the note before, as repeated
//! hits or as one held note. The draws are per gap, so the same bar fills
//! the same way every time it plays.

/// A run of grid points to fill.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Cluster {
    /// The first grid point and how many.
    pub start: usize,
    pub len: usize,
    /// One held note over the run instead of hits.
    pub hold: bool,
}

/// The most clusters a bar gets (a 32-point grid has at most 16 gaps).
pub(crate) const MAX_CLUSTERS: usize = 16;

// MUSICAL: tune by ear. The share of a picked gap a run takes at the lowest
// and the full amount (at least two points where the gap has them); the
// share of runs that lead into the next note (the rest trail the one
// before).
const RUN_SHARE: (f32, f32) = (0.4, 1.0);
const LEAD_IN: f32 = 0.65;
const MIN_RUN: usize = 2;

/// The clusters for a bar whose grid points `free` are rests, at `amount`
/// 0..1 (the share of gaps picked, and how much of each a run takes), a
/// run held with chance `hold`. `unit(point, k)` is the seeded draw `k` for
/// the gap starting at grid point `point`. Returns how many it wrote.
pub(crate) fn plan(free: &[bool], amount: f32, hold: f32, unit: impl Fn(usize, u32) -> f32, out: &mut [Cluster; MAX_CLUSTERS]) -> usize {
    let a = amount.clamp(0.0, 1.0);
    if a <= 0.0 {
        return 0;
    }
    let mut n = 0;
    let mut k = 0;
    while k < free.len() && n < MAX_CLUSTERS {
        if !free[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k < free.len() && free[k] {
            k += 1;
        }
        let gap = k - start;
        // Whole gaps are picked, so a low amount fills a few gaps well
        // rather than many points thinly.
        if a < 1.0 && unit(start, 0) >= a {
            continue;
        }
        let share = RUN_SHARE.0 + (RUN_SHARE.1 - RUN_SHARE.0) * a;
        let len = ((gap as f32 * share).ceil() as usize).max(MIN_RUN.min(gap)).min(gap);
        let lead_in = unit(start, 1) < LEAD_IN;
        out[n] = Cluster { start: if lead_in { start + gap - len } else { start }, len, hold: unit(start, 2) < hold };
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(seed: u32) -> impl Fn(usize, u32) -> f32 {
        move |p, k| crate::rng::unit_f32(crate::rng::hash(seed, 0, p as u8, k as u8, 0xC1))
    }

    #[test]
    fn runs_are_whole_gaps_or_at_least_two_points() {
        // Notes on 0, 4 and 8 of a 16-point grid: three gaps of three and
        // one of seven.
        let free: Vec<bool> = (0..16).map(|k| ![0, 4, 8].contains(&k)).collect();
        for seed in 0..40 {
            for amount in [0.2f32, 0.5, 0.8] {
                let mut out = [Cluster::default(); MAX_CLUSTERS];
                let n = plan(&free, amount, 0.0, draw(seed), &mut out);
                for c in &out[..n] {
                    assert!(c.len >= 2, "{c:?}");
                    assert!((c.start..c.start + c.len).all(|k| free[k]), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn full_amount_fills_every_rest() {
        let free: Vec<bool> = (0..16).map(|k| ![0, 4, 8].contains(&k)).collect();
        let mut out = [Cluster::default(); MAX_CLUSTERS];
        let n = plan(&free, 1.0, 0.0, draw(3), &mut out);
        let filled: usize = out[..n].iter().map(|c| c.len).sum();
        assert_eq!(filled, free.iter().filter(|f| **f).count());
    }

    #[test]
    fn more_amount_fills_more() {
        let free: Vec<bool> = (0..32).map(|k| k % 6 != 0).collect();
        let total = |a: f32| -> usize {
            (0..30)
                .map(|seed| {
                    let mut out = [Cluster::default(); MAX_CLUSTERS];
                    let n = plan(&free, a, 0.0, draw(seed), &mut out);
                    out[..n].iter().map(|c| c.len).sum::<usize>()
                })
                .sum()
        };
        assert!(total(0.3) < total(0.6) && total(0.6) < total(0.9));
    }
}
