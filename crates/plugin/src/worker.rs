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
use mesh_vst_core::kit::{mudpie_bank, share, DrumKit};
use mesh_vst_core::macros::LaneSettings;
use mesh_vst_core::library::Library;
use mesh_vst_core::loader::{self, DrumPicks, Slot};
use mesh_vst_core::pattern::PatternLane;
use mesh_vst_core::sampler::{SampleBank, SampleData};
use mesh_vst_core::slots::PatternBook;
use serde::{Deserialize, Serialize};

use crate::builtin;

/// How often the worker looks for requests.
const POLL: Duration = Duration::from_millis(15);
/// Environment variable with library folders (separated like `PATH`); it
/// replaces the settings file's folders.
pub const LIBRARY_ENV: &str = "MESH_VST_LIBRARY";

/// Which kit to load, and the settings it was asked for.
/// `Copy`, so the audio thread can send it without allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request {
    /// The settings, and the kit generation (raised by library edits and pins).
    Drums(LaneSettings, u32),
    Bass(LaneSettings, u32),
}

/// A kit ready to play, with the lane's output gain (D64: a constant, the
/// notes come matched) and the `settings` it was asked for.
pub enum Ready {
    Drums { lane: Box<PatternLane>, bank: Shared<SampleBank>, loop_bpm: Option<f64>, gain: f32, settings: LaneSettings, generation: u32 },
    Bass { bank: Shared<SampleBank>, gain: f32, settings: LaneSettings, generation: u32 },
    /// A file to preview, on target 0 (D35).
    Audition { bank: Shared<SampleBank> },
    /// The user's patterns changed (D60): both modes' books.
    Patterns { drums: Arc<PatternBook>, bass: Arc<PatternBook> },
}

/// What the audio thread hands back to be dropped here.
pub enum Garbage {
    Lane(Box<PatternLane>),
    Book(Arc<PatternBook>),
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
    /// Files the user pinned to a slot: they stay over seed changes (D21).
    pub pins: BTreeMap<Slot, FileRef>,
    /// Files a slot's own roll picked: they play until the sound seed
    /// changes (user, 2026-10-20: a slot roll does not pin).
    #[serde(default)]
    pub rolled: BTreeMap<Slot, FileRef>,
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

/// The bass clips (D50): the shipped ones and the user's from
/// `~/.config/mesh-vst/clips/a` .. `d` (unreadable files are skipped).
pub fn bass_clips() -> std::sync::Arc<mesh_vst_core::clips::ClipSet> {
    match config_dir().map(|d| d.join("clips")).filter(|d| d.is_dir()) {
        Some(dir) => std::sync::Arc::new(mesh_vst_core::clips::ClipSet::with_dir(&dir).0),
        None => mesh_vst_core::clips::ClipSet::builtin(),
    }
}

pub fn cache_file() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("mesh-vst").join("library.json"))
}

/// Save the per-machine settings.
pub fn save_settings(s: &Settings) -> std::io::Result<()> {
    let dir = config_dir().ok_or_else(|| std::io::Error::other("no config directory"))?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("settings.json"), serde_json::to_vec_pretty(s)?)
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
    pub fn spawn(sample_rate: u32, state: Arc<Mutex<KitState>>, ui: Arc<crate::ui::UiShared>, patterns: Arc<Mutex<crate::patterns::PatternBooks>>) -> Self {
        let (requests, mut req_rx) = rtrb::RingBuffer::new(16);
        let (mut ready_tx, ready) = rtrb::RingBuffer::new(4);
        let (garbage, mut garbage_rx) = rtrb::RingBuffer::new(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let thread = std::thread::Builder::new()
            .name("mesh-vst-loader".into())
            .spawn(move || {
                let mut job = Job::new(sample_rate, state);
                // The books last handed over (none yet: the first poll sends them).
                let mut sent: Option<crate::patterns::PatternBooks> = None;
                while !stop_thread.load(Ordering::Acquire) {
                    while garbage_rx.pop().is_ok() {}
                    // The patterns, when the editor or a loaded project changed them.
                    let books = patterns.lock().map(|b| b.clone()).ok();
                    if let Some(b) = books.filter(|b| sent.as_ref() != Some(b)) {
                        let ready = Ready::Patterns { drums: Arc::new(b.drums.clone()), bass: Arc::new(b.bass.clone()) };
                        if ready_tx.push(ready).is_ok() {
                            sent = Some(b);
                        }
                    }
                    let audition = ui.audition.lock().ok().and_then(|mut a| a.take());
                    if let Some(path) = audition {
                        let root = 60;
                        if let Ok(data) = loader::load_sample(&path, root, sample_rate, 0.0) {
                            let mut bank = SampleBank::new();
                            bank.set(0, mesh_vst_core::sampler::SlotDef::whole(share(data)).unpitched());
                            let _ = ready_tx.push(Ready::Audition { bank: Shared::new(&gc_handle(), bank) });
                        }
                    }
                    // Only the newest request of a burst matters.
                    let mut latest = None;
                    while let Ok(r) = req_rx.pop() {
                        latest = Some(r);
                    }
                    if let Some(r) = latest {
                        if let Some(done) = job.load(r) {
                            // The editor's drum rows follow the kit's layout.
                            if let Ready::Drums { lane, .. } = &done {
                                if let Ok(mut l) = ui.drum_layout.lock() {
                                    *l = Some(*lane.layout());
                                }
                            }
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

/// The worker's state: the library and the kits it last loaded, so a seed
/// or form change only builds the lane again.
struct Job {
    sample_rate: u32,
    state: Arc<Mutex<KitState>>,
    library: Option<Library>,
    drums: Option<(u32, DrumKit, Option<f64>)>,
    bass: Option<(u32, Shared<SampleData>)>,
    /// Kit generation the cache and the library belong to.
    generation: u32,
}

impl Job {
    fn new(sample_rate: u32, state: Arc<Mutex<KitState>>) -> Self {
        Self { sample_rate, state, library: None, drums: None, bass: None, generation: 0 }
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
        let generation = match r {
            Request::Drums(_, g) | Request::Bass(_, g) => g,
        };
        if generation != self.generation {
            // The editor changed the library or a pin: read both again.
            self.generation = generation;
            self.library = None;
            self.drums = None;
            self.bass = None;
        }
        match r {
            Request::Drums(settings, _) => {
                let seed = settings.sound_seed;
                if self.drums.as_ref().is_none_or(|d| d.0 != seed) {
                    let (kit, loop_bpm) = self.load_drums(seed)?;
                    self.drums = Some((seed, kit, loop_bpm));
                }
                let (_, kit, loop_bpm) = self.drums.as_ref()?;
                let (layout, bank) = kit.build();
                let bank = Shared::new(&gc_handle(), bank);
                let gain = crate::LANE_GAIN;
                let lane = Box::new(PatternLane::drums(layout, 1, seed));
                Some(Ready::Drums { lane, bank, loop_bpm: *loop_bpm, gain, settings, generation })
            }
            Request::Bass(settings, _) => {
                let seed = settings.sound_seed;
                if self.bass.as_ref().is_none_or(|b| b.0 != seed) {
                    let sample = self.load_mudpie(seed)?;
                    self.bass = Some((seed, sample));
                }
                let (_, sample) = self.bass.as_ref()?;
                let bank = Shared::new(&gc_handle(), mudpie_bank(sample.clone()));
                let gain = crate::LANE_GAIN;
                Some(Ready::Bass { bank, gain, settings, generation })
            }
        }
    }

    /// The kit of `sound_seed`: the project's files when it was saved with
    /// this seed, else the seed's picks; pins replace either. Built-in sounds
    /// without a library.
    fn load_drums(&mut self, sound_seed: u32) -> Option<(DrumKit, Option<f64>)> {
        let sr = self.sample_rate;
        let state = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        self.library();
        let lib = self.library.as_ref()?;
        let resolve = |f: &FileRef| resolve(lib, f);
        let pins: DrumPicks = state.pins.iter().filter_map(|(s, f)| Some((*s, resolve(f)?))).collect();
        // A new sound seed drops the slots' own rolls (not the pins).
        let same_seed = state.drums_seed == Some(sound_seed);
        let rolled: DrumPicks = if same_seed { state.rolled.iter().filter_map(|(s, f)| Some((*s, resolve(f)?))).collect() } else { DrumPicks::new() };
        // The seed's files (the project's when it was saved with this seed);
        // the saved state keeps them apart from the pins, so unpinning a slot
        // brings the seed's file back.
        let seeded: DrumPicks = if state.drums_seed == Some(sound_seed) && !state.drums.is_empty() {
            state.drums.iter().filter_map(|(s, f)| Some((*s, resolve(f)?))).collect()
        } else {
            loader::pick_drums(lib, sound_seed, &DrumPicks::new())
        };
        let refs: BTreeMap<Slot, FileRef> = seeded.iter().map(|(s, p)| (*s, file_ref(lib, p))).collect();
        let mut picks = seeded;
        picks.extend(rolled);
        picks.extend(pins);
        let (kit, loop_bpm) = if picks.is_empty() {
            (builtin::drum_kit(sr), None)
        } else {
            let loaded = loader::load_drums(lib, &picks, sr);
            (loaded.kit, loaded.break_bpm.map(f64::from))
        };
        if let Ok(mut s) = self.state.lock() {
            s.drums_seed = Some(sound_seed);
            s.drums = refs;
            if !same_seed {
                s.rolled.clear();
            }
        }
        Some((kit, loop_bpm))
    }

    /// The mudpie of `sound_seed`, as [`Self::load_drums`] picks a kit.
    fn load_mudpie(&mut self, sound_seed: u32) -> Option<Shared<SampleData>> {
        let sr = self.sample_rate;
        let state = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        self.library();
        let lib = self.library.as_ref()?;
        let pin = state.mudpie_pin.as_ref().and_then(|f| resolve(lib, f));
        // The seed's mudpie (the project's when saved with this seed), kept
        // apart from the pin like the drum slots.
        let seeded = match (&state.mudpie, state.bass_seed == Some(sound_seed)) {
            (Some(f), true) => resolve(lib, f).or_else(|| loader::pick_mudpie(lib, sound_seed, None)),
            _ => loader::pick_mudpie(lib, sound_seed, None),
        };
        let mudpie = seeded.as_ref().map(|p| file_ref(lib, p));
        let path = pin.or(seeded);
        let sample = path
            .and_then(|p| loader::load_mudpie(lib, &p, sr).ok())
            .unwrap_or_else(|| share(builtin::mudpie(sr)));
        if let Ok(mut s) = self.state.lock() {
            s.bass_seed = Some(sound_seed);
            s.mudpie = mudpie;
        }
        Some(sample)
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
