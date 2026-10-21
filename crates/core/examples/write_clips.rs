//! Write the shipped bass clips (D50) from their tables to
//! `assets/clips/<letter>/NN.mid`.
//!
//! `cargo run -p mesh-vst-core --example write_clips`

use mesh_vst_core::clips::{tables, LETTER_DIRS};
use mesh_vst_core::motif::midi;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/clips");
    for (l, dir) in LETTER_DIRS.iter().enumerate() {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).expect("clip folder");
        for (i, c) in tables::of(l).iter().enumerate() {
            let path = dir.join(format!("{:02}.mid", i + 1));
            std::fs::write(&path, midi::write(&c.clip(), c.name)).expect("clip file");
            println!("{}: {}", path.display(), c.name);
        }
    }
}
