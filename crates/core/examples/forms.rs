//! Print generated forms (D48), one per seed, to read them.
//!
//! ```text
//! cargo run -p mesh-vst-core --example forms -- [first seed] [count] [variation 0..1] [shape 0..1]
//! ```

use mesh_vst_core::arrange::{band, FormBuilder, FormSeeds, FormSpec};
use mesh_vst_core::form::Form;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: f32| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (first, count) = (num(0, 1.0) as u32, num(1, 8.0) as u32);
    let (variation, shape) = (band(num(2, 0.5)), band(num(3, 0.5)));
    let mut builder = FormBuilder::new();
    let mut form = Form::generated();
    for seed in first..first + count {
        let spec = FormSpec { seeds: FormSeeds::new(seed, seed, seed), variation, shape, drums: false };
        builder.build(&spec, &mut form);
        println!("seed {seed} ({}, second: {:?}):", builder.plan_name(), builder.recipe_names());
        let text = form.to_string();
        let (a, b) = text.split_once(" // ").unwrap_or((&text, ""));
        println!("  1: {a}\n  2: {b}");
    }
}
