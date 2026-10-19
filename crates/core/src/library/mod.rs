//! The sample library: folders scanned once, files sorted into pools, the
//! sound seed picking from them (design D17–D19, D30, D31).
//!
//! A scan walks the library folders, classifies every audio file
//! ([`classify`]) and remembers the result by path, size and modification
//! time, so the next scan only reads new or changed files. Pools are
//! suggestions: [`Library::set_override`] moves a file to another pool for
//! good. A file has a content hash, so a project can find a moved sample
//! again ([`Library::find_hash`]).

pub mod classify;

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::rng;
use classify::{Class, Features, ANALYSE_SECS};

/// Where a file plays (design D16, D31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Pool {
    Kick,
    Snare,
    Hats,
    /// Rides, crashes, shakers and other tops.
    Cymbal,
    Perc,
    /// Break and top loops.
    Break,
    /// Long tonal material for the bass.
    Mudpie,
    /// Unsure and FX: the user sorts these by hand.
    Unsorted,
}

impl Pool {
    pub const ALL: [Pool; 8] =
        [Pool::Kick, Pool::Snare, Pool::Hats, Pool::Cymbal, Pool::Perc, Pool::Break, Pool::Mudpie, Pool::Unsorted];

    pub fn name(self) -> &'static str {
        match self {
            Pool::Kick => "kick",
            Pool::Snare => "snare",
            Pool::Hats => "hats",
            Pool::Cymbal => "cymbal",
            Pool::Perc => "perc",
            Pool::Break => "break",
            Pool::Mudpie => "mudpie",
            Pool::Unsorted => "unsorted",
        }
    }

    fn of(class: Option<Class>) -> Pool {
        match class {
            Some(Class::Kick) => Pool::Kick,
            Some(Class::Snare) => Pool::Snare,
            Some(Class::Hat) => Pool::Hats,
            Some(Class::Cymbal) => Pool::Cymbal,
            Some(Class::Perc) => Pool::Perc,
            Some(Class::Loop) => Pool::Break,
            Some(Class::Tonal) => Pool::Mudpie,
            Some(Class::Fx) | None => Pool::Unsorted,
        }
    }
}

/// What a file's name says beyond its pool: the seed prefers these (D31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Tag {
    #[default]
    None,
    OpenHat,
    Ride,
    Crash,
    Shaker,
}

fn tag_of(name: &str, pool: Pool, decay: f32) -> Tag {
    let t = name.to_lowercase();
    let toks: Vec<&str> = t.split(|c: char| !c.is_ascii_alphabetic()).collect();
    match pool {
        Pool::Hats if t.contains("open") || toks.contains(&"oh") || (!t.contains("closed") && decay > 0.35) => Tag::OpenHat,
        Pool::Cymbal if t.contains("ride") => Tag::Ride,
        Pool::Cymbal if t.contains("crash") || t.contains("splash") || t.contains("china") => Tag::Crash,
        Pool::Cymbal if t.contains("shake") || t.contains("tambo") => Tag::Shaker,
        _ => Tag::None,
    }
}

/// One scanned file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime: u64,
    /// Content hash ([`content_hash`]).
    pub hash: u64,
    /// The classifier's pool.
    pub detected: Pool,
    pub tag: Tag,
    pub secs: f32,
    /// Loops: tempo and length in beats.
    pub bpm: Option<f32>,
    pub beats: Option<u32>,
    /// Pitched material: the MIDI key it sounds at.
    pub root: Option<u8>,
}

/// File extensions the library reads.
pub const AUDIO_EXTENSIONS: [&str; 5] = ["wav", "flac", "aif", "aiff", "ogg"];

/// The persisted part of a library: what the scans found, what the user moved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    pub folders: Vec<PathBuf>,
    entries: BTreeMap<PathBuf, Entry>,
    /// Pools the user chose, by path (D30).
    overrides: BTreeMap<PathBuf, Pool>,
}

impl Library {
    pub fn new(folders: Vec<PathBuf>) -> Self {
        Self { folders, ..Default::default() }
    }

    /// Read a saved library (cache plus overrides). A missing or broken file
    /// gives an empty one.
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)
    }

    /// Scan the folders: classify new and changed files, forget gone ones.
    /// Returns how many files were read. Blocking; run it off the audio thread.
    pub fn scan(&mut self) -> usize {
        let mut files = Vec::new();
        for f in &self.folders {
            walk(f, &mut files);
        }
        let mut read = 0;
        let mut entries = BTreeMap::new();
        for path in files {
            let Ok(meta) = fs::metadata(&path) else { continue };
            let size = meta.len();
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            let known = self.entries.get(&path).filter(|e| e.size == size && e.mtime == mtime).cloned();
            let entry = match known {
                Some(e) => e,
                None => {
                    read += 1;
                    match self.analyse(&path, size, mtime) {
                        Some(e) => e,
                        None => continue,
                    }
                }
            };
            entries.insert(path, entry);
        }
        self.entries = entries;
        read
    }

    fn analyse(&self, path: &Path, size: u64, mtime: u64) -> Option<Entry> {
        let (mono, sr, secs) = crate::sampler::decode_mono(path, ANALYSE_SECS).ok()?;
        let f = classify::analyse(&mono, sr, secs);
        let name = self.relative(path);
        let detected = Pool::of(classify::classify(&name, &f));
        let (bpm, beats) = if detected == Pool::Break { loop_tempo(&name, secs, &f) } else { (None, None) };
        let root = (detected == Pool::Mudpie).then(|| root_key(&name, &f)).flatten();
        Some(Entry {
            path: path.to_path_buf(),
            size,
            mtime,
            hash: content_hash(path).unwrap_or(0),
            detected,
            tag: tag_of(&name, detected, f.decay),
            secs,
            bpm,
            beats,
            root,
        })
    }

    /// `path` below its library folder, for names and project state.
    pub fn relative(&self, path: &Path) -> String {
        self.folders
            .iter()
            .find_map(|f| path.strip_prefix(f).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, path: &Path) -> Option<&Entry> {
        self.entries.get(path)
    }

    /// The pool a file plays in: the user's choice, else the classifier's.
    pub fn pool_of(&self, e: &Entry) -> Pool {
        self.overrides.get(&e.path).copied().unwrap_or(e.detected)
    }

    /// Move a file to `pool` for good (D30). `None` goes back to the classifier.
    pub fn set_override(&mut self, path: &Path, pool: Option<Pool>) {
        match pool {
            Some(p) => self.overrides.insert(path.to_path_buf(), p),
            None => self.overrides.remove(path),
        };
    }

    /// The files of a pool, sorted by path.
    pub fn pool(&self, pool: Pool) -> Vec<&Entry> {
        self.entries.values().filter(|e| self.pool_of(e) == pool).collect()
    }

    /// Files per pool.
    pub fn counts(&self) -> BTreeMap<Pool, usize> {
        let mut out = BTreeMap::new();
        for e in self.entries.values() {
            *out.entry(self.pool_of(e)).or_insert(0) += 1;
        }
        out
    }

    /// The file `seed` picks for `slot` in `pool`, preferring files tagged
    /// `prefer` when there are some. Deterministic for a given library.
    pub fn pick(&self, pool: Pool, seed: u32, slot: u8, prefer: Option<Tag>) -> Option<&Entry> {
        let all = self.pool(pool);
        let tagged: Vec<&Entry> = match prefer {
            Some(t) => all.iter().copied().filter(|e| e.tag == t).collect(),
            None => Vec::new(),
        };
        let from = if tagged.is_empty() { &all } else { &tagged };
        if from.is_empty() {
            return None;
        }
        let h = rng::hash(seed, slot, pool as u8, 0, S_PICK);
        Some(from[(h % from.len() as u64) as usize])
    }

    /// A file with this content hash, wherever it is now (D18).
    pub fn find_hash(&self, hash: u64) -> Option<&Entry> {
        (hash != 0).then(|| self.entries.values().find(|e| e.hash == hash)).flatten()
    }

    /// Copy a file from outside the library into `<first folder>/mesh vst
    /// imports/` (D19) and scan it. A file already inside a folder stays.
    pub fn import(&mut self, file: &Path) -> std::io::Result<PathBuf> {
        if self.folders.iter().any(|f| file.starts_with(f)) {
            self.scan_one(file);
            return Ok(file.to_path_buf());
        }
        let dir = self.folders.first().ok_or_else(|| std::io::Error::other("no library folder"))?.join(IMPORTS_DIR);
        fs::create_dir_all(&dir)?;
        let name = file.file_name().ok_or_else(|| std::io::Error::other("not a file"))?;
        let mut dest = dir.join(name);
        let mut n = 1;
        while dest.exists() && content_hash(&dest).ok() != content_hash(file).ok() {
            n += 1;
            let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ext = file.extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            dest = dir.join(format!("{stem} {n}.{ext}"));
        }
        if !dest.exists() {
            fs::copy(file, &dest)?;
        }
        self.scan_one(&dest);
        Ok(dest)
    }

    fn scan_one(&mut self, path: &Path) {
        let Ok(meta) = fs::metadata(path) else { return };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        if let Some(e) = self.analyse(path, meta.len(), mtime) {
            self.entries.insert(path.to_path_buf(), e);
        }
    }
}

/// Folder dropped files from outside the library are copied to.
pub const IMPORTS_DIR: &str = "mesh vst imports";
const S_PICK: u32 = 0x5A0;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| e.eq_ignore_ascii_case(a))) {
            out.push(p);
        }
    }
}

/// FNV-1a over the size and the first and last 64 KiB: stable across
/// machines and fast on large files.
pub fn content_hash(path: &Path) -> std::io::Result<u64> {
    use std::io::{Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(&len.to_le_bytes());
    let mut buf = vec![0u8; CHUNK.min(len) as usize];
    f.read_exact(&mut buf)?;
    eat(&buf);
    if len > CHUNK {
        let tail = CHUNK.min(len - CHUNK);
        f.seek(SeekFrom::End(-(tail as i64)))?;
        let mut buf = vec![0u8; tail as usize];
        f.read_exact(&mut buf)?;
        eat(&buf);
    }
    Ok(h)
}

/// A loop's tempo and length in beats: a tempo in its name when the length
/// fits it, else the length that puts it at a usual break tempo, else the
/// detected tempo.
fn loop_tempo(name: &str, secs: f32, f: &Features) -> (Option<f32>, Option<u32>) {
    let fits = |bpm: f32| {
        let beats = secs * bpm / 60.0;
        let r = beats.round();
        (r >= 1.0 && (beats - r).abs() / r < 0.03).then_some(r as u32)
    };
    // A number 60..=200 in the name.
    let numbers = name.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse::<f32>().ok());
    for bpm in numbers.filter(|b| (60.0..=200.0).contains(b)) {
        if let Some(beats) = fits(bpm) {
            return (Some(bpm), Some(beats));
        }
    }
    // MUSICAL: breaks mostly sit between 160 and 180 bpm, then 120..190.
    for (lo, hi) in [(160.0, 180.0), (120.0, 190.0)] {
        for beats in [4u32, 8, 16, 2, 32] {
            let bpm = beats as f32 * 60.0 / secs;
            if (lo..=hi).contains(&bpm) {
                return (Some(bpm), Some(beats));
            }
        }
    }
    if f.bpm > 0.0 {
        return (Some(f.bpm), fits(f.bpm));
    }
    (None, None)
}

/// The key a tonal file sounds at: a note name in its file name (`F#1`,
/// `_C2_`), else its detected pitch.
fn root_key(name: &str, f: &Features) -> Option<u8> {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    for tok in stem.split(|c: char| c == '_' || c == ' ' || c == '-' || c == '.') {
        if let Some(k) = parse_note(tok) {
            return Some(k);
        }
    }
    (f.pitch_hz > 20.0).then(|| (69.0 + 12.0 * (f.pitch_hz / 440.0).log2()).round().clamp(0.0, 127.0) as u8)
}

/// `C2`, `F#1`, `Bb0`, with octave numbers where C4 = 60.
fn parse_note(tok: &str) -> Option<u8> {
    let b = tok.as_bytes();
    let pc: i32 = match b.first()?.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let (acc, rest) = match b.get(1) {
        Some(b'#') => (1, &tok[2..]),
        Some(b'b') if b.len() > 2 => (-1, &tok[2..]),
        _ => (0, &tok[1..]),
    };
    let octave: i32 = rest.parse().ok().filter(|o| (-1..=8).contains(o))?;
    u8::try_from((octave + 1) * 12 + pc + acc).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names() {
        assert_eq!(parse_note("C4"), Some(60));
        assert_eq!(parse_note("F#1"), Some(30));
        assert_eq!(parse_note("Bb0"), Some(22));
        assert_eq!(parse_note("Break"), None);
        assert_eq!(parse_note("A"), None);
    }

    #[test]
    fn loop_tempo_from_name_or_length() {
        let f = Features::default();
        // 170 bpm, 4 beats = 1.412 s.
        assert_eq!(loop_tempo("170_ThursdayBreak_SP_01.wav", 1.4118, &f), (Some(170.0), Some(4)));
        // No tempo in the name: 2.824 s as 8 beats is 170 bpm.
        let (bpm, beats) = loop_tempo("amen.wav", 2.8235, &f);
        assert_eq!(beats, Some(8));
        assert!((bpm.unwrap() - 170.0).abs() < 0.1);
    }

    #[test]
    fn picks_are_deterministic_and_prefer_tags() {
        let mut lib = Library::new(vec![PathBuf::from("/lib")]);
        for (i, (pool, tag)) in [(Pool::Cymbal, Tag::Ride), (Pool::Cymbal, Tag::Crash), (Pool::Cymbal, Tag::Crash), (Pool::Kick, Tag::None)]
            .into_iter()
            .enumerate()
        {
            let path = PathBuf::from(format!("/lib/{i}.wav"));
            let e = Entry { path: path.clone(), size: 1, mtime: 1, hash: i as u64 + 1, detected: pool, tag, secs: 1.0, bpm: None, beats: None, root: None };
            lib.entries.insert(path, e);
        }
        let a = lib.pick(Pool::Cymbal, 5, 0, None).map(|e| e.path.clone());
        assert_eq!(a, lib.pick(Pool::Cymbal, 5, 0, None).map(|e| e.path.clone()));
        assert_eq!(lib.pick(Pool::Cymbal, 9, 1, Some(Tag::Ride)).unwrap().tag, Tag::Ride);
        assert!(lib.pick(Pool::Perc, 1, 0, None).is_none());
        // An override moves a file for good.
        lib.set_override(Path::new("/lib/3.wav"), Some(Pool::Perc));
        assert_eq!(lib.pool(Pool::Perc).len(), 1);
        assert_eq!(lib.find_hash(4).unwrap().path, PathBuf::from("/lib/3.wav"));
    }
}
