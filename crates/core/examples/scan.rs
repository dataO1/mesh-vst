//! Scan a library folder and report the pools.
//!
//! ```text
//! cargo run -p mesh-vst-core --release --example scan -- <folder> [--cache lib.json] [--truth]
//! ```
//!
//! `--truth` treats the folder's top-level folder names as the answer
//! (Kicks, Snares, Cymbals, Percussion, Breaks, FX, Mudpies, Synths, as in
//! the S1 spike) and reports how often the classifier, with those folder
//! names hidden, agrees.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use mesh_vst_core::library::classify::{self, ANALYSE_SECS};
use mesh_vst_core::library::{Library, Pool};
use mesh_vst_core::sampler::decode_mono;

fn truth(rel: &str) -> Option<Pool> {
    let top = rel.split('/').next()?.to_lowercase();
    let name = rel.rsplit('/').next()?.to_lowercase();
    Some(match top.as_str() {
        "kicks" => Pool::Kick,
        "snares" => Pool::Snare,
        "breaks" => Pool::Break,
        "fx" => Pool::Unsorted,
        "mudpies" | "synths" => Pool::Mudpie,
        "percussion" if name.contains("loop") => Pool::Break,
        "percussion" => Pool::Perc,
        "cymbals" if name.contains("loop") => Pool::Break,
        "cymbals" if ["crash", "ride", "cymbal", "shaker"].iter().any(|k| name.contains(k)) => Pool::Cymbal,
        "cymbals" if ["hat", "hh"].iter().any(|k| name.contains(k)) => Pool::Hats,
        _ => return None,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let folder = PathBuf::from(args.next().expect("usage: scan <folder> [--cache f] [--truth]"));
    let rest: Vec<String> = args.collect();
    let cache = rest.iter().position(|a| a == "--cache").and_then(|i| rest.get(i + 1)).map(PathBuf::from);
    let want_truth = rest.iter().any(|a| a == "--truth");

    let mut lib = cache.as_deref().map(Library::load).unwrap_or_default();
    lib.folders = vec![folder.clone()];
    let t = Instant::now();
    let read = lib.scan();
    println!("{} files, {read} read, {:.2} s", lib.len(), t.elapsed().as_secs_f32());
    if let Some(c) = &cache {
        lib.save(c).expect("save cache");
    }
    for (pool, n) in lib.counts() {
        println!("  {:9} {n}", pool.name());
    }

    if want_truth {
        let (mut right, mut unsure, mut total) = (0usize, 0usize, 0usize);
        let mut per: BTreeMap<Pool, (usize, usize)> = BTreeMap::new();
        for pool in Pool::ALL {
            for e in lib.pool(pool) {
                let rel = lib.relative(&e.path);
                let Some(want) = truth(&rel) else { continue };
                // Classify again with the top folder hidden.
                let hidden = rel.split_once('/').map_or(rel.as_str(), |x| x.1).to_string();
                let Ok((mono, sr, secs)) = decode_mono(&e.path, ANALYSE_SECS) else { continue };
                let f = classify::analyse(&mono, sr, secs);
                let got = classify::classify(&hidden, &f);
                let got_pool = match got {
                    Some(classify::Class::Kick) => Some(Pool::Kick),
                    Some(classify::Class::Snare) => Some(Pool::Snare),
                    Some(classify::Class::Hat) => Some(Pool::Hats),
                    Some(classify::Class::Cymbal) => Some(Pool::Cymbal),
                    Some(classify::Class::Perc) => Some(Pool::Perc),
                    Some(classify::Class::Loop) => Some(Pool::Break),
                    Some(classify::Class::Tonal) => Some(Pool::Mudpie),
                    Some(classify::Class::Fx) => Some(Pool::Unsorted),
                    None => None,
                };
                total += 1;
                let slot = per.entry(want).or_default();
                slot.1 += 1;
                match got_pool {
                    None => unsure += 1,
                    Some(p) if p == want => {
                        right += 1;
                        slot.0 += 1;
                    }
                    Some(_) => {}
                }
            }
        }
        let answered = total - unsure;
        println!(
            "folder names hidden: {right}/{total} right ({:.1} %), {unsure} unsure ({:.1} %), {:.1} % of answers right",
            100.0 * right as f32 / total as f32,
            100.0 * unsure as f32 / total as f32,
            100.0 * right as f32 / answered.max(1) as f32
        );
        for (pool, (r, n)) in per {
            println!("  {:9} {r}/{n} ({:.1} %)", pool.name(), 100.0 * r as f32 / n as f32);
        }
    }
}
