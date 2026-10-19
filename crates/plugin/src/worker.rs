//! Loads kits off the audio thread (design D17–D21).
//!
//! The audio thread asks for the kit of a sound seed with a small `Copy`
//! request; the worker scans the library (once, then from its cache), picks
//! the files, loads and normalizes them, builds the bank and the pattern
//! lane, and hands them back through a ring buffer. What the audio thread
//! replaces comes back to the worker to be dropped there.
//!
//! The project keeps the files it played ([`KitState`]): reopening a project
//! plays the same samples even after the library changed, and a moved file
//! is found again by its content hash (D18).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use basedrop::Shared;
use mesh_vst_core::gc::gc_handle;
use mesh_vst_core::kit::{mudpie_bank, share};
use mesh_vst_core::library::Library;
use mesh_vst_core::loader::{self, DrumPicks, Slot};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::sampler::{SampleBank, SampleData};
use serde::{Deserialize, Serialize};

use crate::builtin;

/// How often the worker looks for requests.
const POLL: Duration = Duration::from_millis(15);
/// Environment variable with library folders (separated like `PATH`); it
/// replaces the settings file's folders.
pub const LIBRARY_ENV: &str = "MESH_VST_LIBRARY";

/// Which kit to load. `Copy`, so the audio thread can send it without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Drums { sound_seed: u32 },
    Bass { sound_seed: u32 },
}

/// A kit ready to play.
pub enum Ready {
    Drums { lane: Box<PatternLane>, bank: Shared<SampleBank>, loop_bpm: Option<f64>, sound_seed: u32, builtin: bool },
    Bass { bank: Shared<SampleBank>, sound_seed: u32, builtin: bool },
}

/// What the audio thread hands back to be dropped here.
pub enum Garbage {
    Lane(Box<PatternLane>),
}

/// A file the project played: its path below the library folder and its hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub path: String,
    pub hash: u64,
}

/// The samples the project plays, saved with it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitState {
    /// The sound seed `drums` was picked for.
    pub drums_seed: Option<u32>,
    pub drums: BTreeMap<Slot, FileRef>,
    pub bass_seed: Option<u32>,
    pub mudpie: Option<FileRef>,
    /// Files the user put on a slot by hand: they stay over seed changes (D21).
    pub pins: BTreeMap<Slot, FileRef>,
    pub mudpie_pin: Option<FileRef>,
}

/// Per-machine settings (D17): the library folders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub library_folders: Vec<PathBuf>,
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mesh-vst"))
}

fn cache_file() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("mesh-vst").join("library.json"))
}

/// Library folders: the environment variable, else the settings file.
pub fn library_folders() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os(LIBRARY_ENV) {
        return std::env::split_paths(&v).filter(|p| p.is_dir()).collect();
    }
    config_dir()
        .and_then(|d| std::fs::read(d.join("settings.json")).ok())
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .map(|s| s.library_folders)
        .unwrap_or_default()
}

/// The worker thread and its channels' audio-thread ends.
pub struct Worker {
    pub requests: rtrb::Producer<Request>,
    pub ready: rtrb::Consumer<Ready>,
    pub garbage: rtrb::Producer<Garbage>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(sample_rate: u32, state: Arc<Mutex<KitState>>) -> Self {
        let (requests, mut req_rx) = rtrb::RingBuffer::new(16);
        let (mut ready_tx, ready) = rtrb::RingBuffer::new(4);
        let (garbage, mut garbage_rx) = rtrb::RingBuffer::new(16);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let thread = std::thread::Builder::new()
            .name("mesh-vst-loader".into())
            .spawn(move || {
                let mut job = Job::new(sample_rate, state);
                while !stop_thread.load(Ordering::Acquire) {
                    while garbage_rx.pop().is_ok() {}
                    // Only the newest request of a burst matters.
                    let mut latest = None;
                    while let Ok(r) = req_rx.pop() {
                        latest = Some(r);
                    }
                    if let Some(r) = latest {
                        if let Some(done) = job.load(r) {
                            // The audio thread drains this every block; wait for room.
                            let mut done = Some(done);
                            while let Some(d) = done.take() {
                                if let Err(rtrb::PushError::Full(d)) = ready_tx.push(d) {
                                    done = Some(d);
                                    if stop_thread.load(Ordering::Acquire) {
                                        break;
                                    }
                                    std::thread::sleep(POLL);
                                }
                            }
                        }
                    }
                    std::thread::sleep(POLL);
                }
            })
            .expect("spawn the loader thread");
        Self { requests, ready, garbage, stop, thread: Some(thread) }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The worker's state: the library and what it last loaded.
struct Job {
    sample_rate: u32,
    state: Arc<Mutex<KitState>>,
    library: Option<Library>,
}

impl Job {
    fn new(sample_rate: u32, state: Arc<Mutex<KitState>>) -> Self {
        Self { sample_rate, state, library: None }
    }

    /// The library, scanned on first use.
    fn library(&mut self) -> &Library {
        self.library.get_or_insert_with(|| {
            let cache = cache_file();
            let mut lib = cache.as_deref().map(Library::load).unwrap_or_default();
            lib.folders = library_folders();
            if !lib.folders.is_empty() {
                lib.scan();
                if let Some(c) = &cache {
                    let _ = lib.save(c);
                }
            }
            lib
        })
    }

    fn load(&mut self, r: Request) -> Option<Ready> {
        let sr = self.sample_rate;
        let state = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        self.library();
        let lib = self.library.as_ref()?;
        match r {
            Request::Drums { sound_seed } => {
                let resolve = |f: &FileRef| resolve(lib, f);
                let pins: DrumPicks = state.pins.iter().filter_map(|(s, f)| Some((*s, resolve(f)?))).collect();
                // The project's own files when it was saved with this seed.
                let picks: DrumPicks = if state.drums_seed == Some(sound_seed) && !state.drums.is_empty() {
                    let mut p: DrumPicks = state.drums.iter().filter_map(|(s, f)| Some((*s, resolve(f)?))).collect();
                    p.extend(pins.clone());
                    p
                } else {
                    loader::pick_drums(lib, sound_seed, &pins)
                };
                let builtin = picks.is_empty();
                let (kit, loop_bpm, refs) = if builtin {
                    (builtin::drum_kit(sr), None, BTreeMap::new())
                } else {
                    let loaded = loader::load_drums(lib, &picks, sr);
                    let refs = picks.iter().map(|(s, p)| (*s, file_ref(lib, p))).collect();
                    (loaded.kit, loaded.break_bpm.map(f64::from), refs)
                };
                let (layout, bank) = kit.build();
                if let Ok(mut s) = self.state.lock() {
                    s.drums_seed = Some(sound_seed);
                    s.drums = refs;
                }
                let lane = Box::new(PatternLane::drums(layout, 1, sound_seed));
                Some(Ready::Drums { lane, bank: Shared::new(&gc_handle(), bank), loop_bpm, sound_seed, builtin })
            }
            Request::Bass { sound_seed } => {
                let pin = state.mudpie_pin.as_ref().and_then(|f| resolve(lib, f));
                let path = match (&state.mudpie, state.bass_seed == Some(sound_seed)) {
                    (Some(f), true) => resolve(lib, f).or_else(|| loader::pick_mudpie(lib, sound_seed, pin.as_deref())),
                    _ => loader::pick_mudpie(lib, sound_seed, pin.as_deref()),
                };
                let loaded: Option<(Shared<SampleData>, FileRef)> =
                    path.and_then(|p| loader::load_mudpie(lib, &p, sr).ok().map(|s| (s, file_ref(lib, &p))));
                let (sample, mudpie) = match loaded {
                    Some((s, f)) => (s, Some(f)),
                    None => (share(builtin::mudpie(sr)), None),
                };
                let builtin = mudpie.is_none();
                if let Ok(mut s) = self.state.lock() {
                    s.bass_seed = Some(sound_seed);
                    s.mudpie = mudpie;
                }
                Some(Ready::Bass { bank: Shared::new(&gc_handle(), mudpie_bank(sample)), sound_seed, builtin })
            }
        }
    }
}

fn file_ref(lib: &Library, path: &Path) -> FileRef {
    FileRef { path: lib.relative(path), hash: lib.entry(path).map_or(0, |e| e.hash) }
}

/// Where a saved file is now: below a library folder at its path, else
/// wherever a file with its hash is.
fn resolve(lib: &Library, f: &FileRef) -> Option<PathBuf> {
    lib.folders
        .iter()
        .map(|d| d.join(&f.path))
        .find(|p| p.is_file())
        .or_else(|| lib.find_hash(f.hash).map(|e| e.path.clone()))
}
