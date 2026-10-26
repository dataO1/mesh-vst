//! Print generated forms (D48), one per seed, to read them.
//!
//! ```text
//! cargo run -p mesh-vst-core --example forms -- [first seed] [count] [variation 0..1] [shape 0..1] [stats] [contrast 0..1]
//! ```

use mesh_vst_core::arrange::{band, FormBuilder, FormSeeds, FormSpec};
use mesh_vst_core::form::Form;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: f32| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (first, count) = (num(0, 1.0) as u32, num(1, 8.0) as u32);
    let (variation, shape) = (band(num(2, 0.5)), band(num(3, 0.5)));
    let stats = args.get(4).is_some_and(|a| a != "-");
    let contrast = band(num(5, 0.5));
    let mut builder = FormBuilder::new();
    let mut form = Form::generated();
    let (mut reps, mut broken, mut retried) = (Vec::new(), 0, 0);
    for seed in first..first + count {
        let spec = FormSpec { seeds: FormSeeds::new(seed, seed, seed), variation, shape, contrast, drums: false };
        builder.build(&spec, &mut form);
        let (b, attempt) = builder.broken();
        reps.push(builder.repetition());
        broken += (b != 0) as u32;
        retried += (attempt > 0) as u32;
        if stats {
            continue;
        }
        println!("seed {seed} ({}, motives {:?} bars, cycles {:?}, second: {:?}, repetition {:.2}, broken {b:#b}, attempt {attempt}):", builder.plan_name(), builder.motive_bars(), builder.cycle_beats(), builder.recipe_names(), builder.repetition());
        let text = form.to_string();
        let (a, b) = text.split_once(" // ").unwrap_or((&text, ""));
        println!("  1: {a}\n  2: {b}");
    }
    reps.sort_by(f32::total_cmp);
    let q = |p: f32| reps[((reps.len() - 1) as f32 * p) as usize];
    println!("repetition: 10% {:.2}, median {:.2}, 90% {:.2}; {broken} of {count} break a rule, {retried} built again", q(0.1), q(0.5), q(0.9));
}
