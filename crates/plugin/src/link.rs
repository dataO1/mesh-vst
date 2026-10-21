//! Splice link (design D42): mesh instances see each other through a small
//! shared-memory file, whether the host runs them in one process or each in
//! its own sandbox.
//!
//! Each instance claims one slot and writes, every block, what it plays in
//! the next song bars ([`Occupancy`]), a coarse level per frequency band
//! (for the overlap view), a label and a heartbeat. A follower reads its
//! host's slot. Writes are guarded by a sequence number (odd while
//! writing), so a reader never takes a half-written slot. No locks, no
//! allocation after [`Link::open`]: RT-safe.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mesh_vst_core::splice::Occupancy;
use mesh_vst_core::STEPS_PER_BAR;

/// Instances that can see each other at once.
pub const SLOTS: usize = 32;
/// Song bars a slot carries (the playing one and the next ones).
pub const BARS: usize = 4;
/// Frequency bands of the overlap view.
pub const BANDS: usize = 16;
/// The host id meaning "the sidechain input".
pub const SIDECHAIN: u64 = u64::MAX;
/// A slot whose heartbeat is older than this is free again.
const STALE_MS: u64 = 3000;
// v2 (2026-10-21): the bars carry a pitched lane's line and the form position (D46).
// v3 (2026-10-21): and the seeds the form is built from (D48); v2 instances
// do not see v3 ones.
const MAGIC: u64 = 0x4d45_5348_4c4e_4b33; // "MESHLNK3"

/// What one instance shares.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SlotData {
    pub bars: [Occupancy; BARS],
    /// The instance this one follows (0 = none), so two never follow each other.
    pub follows: u64,
    /// Level per band, 0..255.
    pub bands: [u8; BANDS],
    /// UTF-8, zero padded ("drums 7443").
    pub label: [u8; 24],
}

impl Default for SlotData {
    fn default() -> Self {
        Self { bars: [Occupancy::default(); BARS], follows: 0, bands: [0; BANDS], label: [0; 24] }
    }
}

#[repr(C)]
struct Slot {
    seq: AtomicU32,
    _pad: u32,
    id: AtomicU64,
    heartbeat: AtomicU64,
    data: std::cell::UnsafeCell<SlotData>,
}

#[repr(C)]
struct Table {
    magic: AtomicU64,
    slots: [Slot; SLOTS],
}

/// Another instance, for the host list.
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub id: u64,
    pub label: String,
    pub follows: u64,
}

/// This instance's view of the shared table.
pub struct Link {
    _map: memmap2::MmapMut,
    table: *const Table,
    id: u64,
    slot: Option<usize>,
}

// SAFETY: the table lives in the mapping `Link` owns; every shared field is
// an atomic or guarded by the slot's sequence number.
unsafe impl Send for Link {}
unsafe impl Sync for Link {}

/// The shared file: /dev/shm on Linux (memory only), the temp dir elsewhere.
fn path() -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    let dir = if shm.is_dir() { shm } else { std::env::temp_dir() };
    dir.join("mesh-vst-link-v3")

}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Link {
    /// Map the table, creating it when it is not there. Not RT-safe.
    pub fn open(id: u64) -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path())?;
        let size = std::mem::size_of::<Table>() as u64;
        if file.metadata()?.len() < size {
            file.set_len(size)?;
        }
        // SAFETY: the file is ours to map; other instances only touch it
        // through the same atomics.
        let map = unsafe { memmap2::MmapMut::map_mut(&file)? };
        let table = map.as_ptr() as *const Table;
        let link = Self { _map: map, table, id, slot: None };
        let t = link.table();
        if t.magic.load(Ordering::Acquire) != MAGIC {
            // A fresh file is all zeros: every slot is free already.
            t.magic.store(MAGIC, Ordering::Release);
        }
        Ok(link)
    }

    fn table(&self) -> &Table {
        // SAFETY: the mapping lives as long as `self` and is large enough.
        unsafe { &*self.table }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Claim a slot (the one already ours, a free one, or a stale one).
    fn claim(&mut self, now: u64) -> Option<usize> {
        if let Some(s) = self.slot.filter(|&s| self.table().slots[s].id.load(Ordering::Acquire) == self.id) {
            return Some(s);
        }
        let t = self.table();
        for (i, s) in t.slots.iter().enumerate() {
            let id = s.id.load(Ordering::Acquire);
            let stale = now.saturating_sub(s.heartbeat.load(Ordering::Acquire)) > STALE_MS;
            if (id == 0 || stale || id == self.id) && s.id.compare_exchange(id, self.id, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                s.heartbeat.store(now, Ordering::Release);
                self.slot = Some(i);
                return Some(i);
            }
        }
        None
    }

    /// Publish this instance's data. RT-safe.
    pub fn publish(&mut self, data: &SlotData) {
        let now = now_ms();
        let Some(i) = self.claim(now) else { return };
        let s = &self.table().slots[i];
        let seq = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq.wrapping_add(1) | 1, Ordering::Release);
        std::sync::atomic::fence(Ordering::Release);
        // SAFETY: we own the slot; readers retry while `seq` is odd or moved.
        unsafe { std::ptr::write_volatile(s.data.get(), *data) };
        std::sync::atomic::fence(Ordering::Release);
        s.seq.store((seq | 1).wrapping_add(1), Ordering::Release);
        s.heartbeat.store(now, Ordering::Release);
    }

    /// The data of instance `id`, when it is alive. RT-safe (a few retries
    /// at most).
    pub fn read(&self, id: u64) -> Option<SlotData> {
        let now = now_ms();
        let s = self.table().slots.iter().find(|s| s.id.load(Ordering::Acquire) == id)?;
        if now.saturating_sub(s.heartbeat.load(Ordering::Acquire)) > STALE_MS {
            return None;
        }
        for _ in 0..4 {
            let a = s.seq.load(Ordering::Acquire);
            if a & 1 == 1 {
                continue;
            }
            std::sync::atomic::fence(Ordering::Acquire);
            // SAFETY: a plain copy; it is thrown away if a write overlapped.
            let data = unsafe { std::ptr::read_volatile(s.data.get()) };
            std::sync::atomic::fence(Ordering::Acquire);
            if s.seq.load(Ordering::Acquire) == a {
                return Some(data);
            }
        }
        None
    }

    /// The other live instances. Not RT-safe (allocates).
    pub fn peers(&self) -> Vec<Peer> {
        let now = now_ms();
        let mut out = Vec::new();
        for s in &self.table().slots {
            let id = s.id.load(Ordering::Acquire);
            if id == 0 || id == self.id || now.saturating_sub(s.heartbeat.load(Ordering::Acquire)) > STALE_MS {
                continue;
            }
            if let Some(d) = self.read(id) {
                let end = d.label.iter().position(|b| *b == 0).unwrap_or(d.label.len());
                out.push(Peer { id, label: String::from_utf8_lossy(&d.label[..end]).into_owned(), follows: d.follows });
            }
        }
        out
    }

    /// Give the slot back (the instance closes).
    pub fn release(&mut self) {
        if let Some(i) = self.slot.take() {
            let _ = self.table().slots[i].id.compare_exchange(self.id, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.release();
    }
}

/// A label for a slot: "<mode> <sound seed> #<id>", the short id telling
/// apart two instances of the same settings. No allocation (RT-safe).
pub fn label(mode: &str, sound_seed: i32, id: u64) -> [u8; 24] {
    let mut out = [0u8; 24];
    let mut n = 0;
    let mut put = |b: u8| {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    };
    for b in mode.bytes() {
        put(b);
    }
    put(b' ');
    let seed = sound_seed.unsigned_abs();
    for d in [1000, 100, 10, 1] {
        put(b'0' + ((seed / d) % 10) as u8);
    }
    put(b' ');
    put(b'#');
    let hex = b"0123456789abcdef";
    put(hex[((id >> 4) & 0xf) as usize]);
    put(hex[(id & 0xf) as usize]);
    out
}

/// What a sidechain plays, by song bar: the peak level per 16th of the bar
/// that just ended stands for the next bars too (the beat repeats).
#[derive(Debug, Clone, Default)]
pub struct SidechainOcc {
    bar: Option<u64>,
    peaks: [f32; STEPS_PER_BAR],
    last: Option<(u64, [f32; STEPS_PER_BAR])>,
}

impl SidechainOcc {
    /// Feed the peak of a stretch of the sidechain at song tick `tick`.
    pub fn feed(&mut self, tick: f64, peak: f32) {
        let bar_ticks = mesh_vst_core::TICKS_PER_BAR as f64;
        let bar = (tick / bar_ticks).floor().max(0.0) as u64;
        if self.bar != Some(bar) {
            if let Some(b) = self.bar {
                self.last = Some((b, self.peaks));
            }
            self.bar = Some(bar);
            self.peaks = [0.0; STEPS_PER_BAR];
        }
        let step = ((tick.rem_euclid(bar_ticks)) / mesh_vst_core::TICKS_PER_STEP as f64) as usize;
        let p = &mut self.peaks[step.min(STEPS_PER_BAR - 1)];
        *p = p.max(peak);
    }

    /// Occupancy for the bars from `bar` on: the last whole bar, scaled to its loudest step.
    pub fn bars(&self, bar: u64) -> [Occupancy; BARS] {
        let mut out = [Occupancy::default(); BARS];
        let Some((_, peaks)) = self.last else { return out };
        let top = peaks.iter().cloned().fold(0.0, f32::max).max(1e-6);
        let occ = peaks.map(|p| p / top);
        for (k, o) in out.iter_mut().enumerate() {
            *o = mesh_vst_core::splice::pack(bar + k as u64, &occ);
        }
        out
    }
}

/// Lowest and highest band centre (as the visual's bins: 40 Hz .. 16 kHz).
const BAND_RANGE: (f32, f32) = (40.0, 16_000.0);
/// dBFS shown as 0 and 255.
const BAND_DB: (f32, f32) = (-60.0, -6.0);

/// A band's place 0..1 in the visual's bins (log frequency).
pub fn band_pos(b: usize) -> f32 {
    (b as f32 + 0.5) / BANDS as f32
}

/// The level per frequency band a slot shares, metered on the audio
/// thread: band-passes, one follower each. RT-safe.
pub struct BandMeter {
    coef: [(f32, f32, f32, f32); BANDS],
    z: [[f32; 2]; BANDS],
    env: [f32; BANDS],
    release: f32,
}

impl BandMeter {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let top = BAND_RANGE.1.min(0.45 * sr);
        let coef = std::array::from_fn(|b| {
            let hz = BAND_RANGE.0 * (top / BAND_RANGE.0).powf(band_pos(b));
            let w = std::f32::consts::TAU * hz / sr;
            let alpha = w.sin() / (2.0 * 2.0);
            let a0 = 1.0 + alpha;
            (alpha / a0, -alpha / a0, -2.0 * w.cos() / a0, (1.0 - alpha) / a0)
        });
        Self { coef, z: [[0.0; 2]; BANDS], env: [0.0; BANDS], release: (-1.0 / (0.15 * sr)).exp() }
    }

    pub fn process(&mut self, buf: &[mesh_vst_core::types::StereoSample]) {
        for s in buf {
            let x = 0.5 * (s.left + s.right);
            for b in 0..BANDS {
                let (b0, b2, a1, a2) = self.coef[b];
                let z = &mut self.z[b];
                let y = b0 * x + z[0];
                z[0] = -a1 * y + z[1];
                z[1] = b2 * x - a2 * y;
                let a = y.abs();
                let e = &mut self.env[b];
                *e = if a > *e { a } else { a + (*e - a) * self.release };
                if *e < 1e-12 {
                    *e = 0.0;
                }
                if z[0].abs() < 1e-20 {
                    z[0] = 0.0;
                }
                if z[1].abs() < 1e-20 {
                    z[1] = 0.0;
                }
            }
        }
    }

    pub fn levels(&self) -> [u8; BANDS] {
        self.env.map(|e| {
            let db = 20.0 * e.max(1e-9).log10();
            (((db - BAND_DB.0) / (BAND_DB.1 - BAND_DB.0)).clamp(0.0, 1.0) * 255.0) as u8
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_links_see_each_other_and_share_bars() {
        let mut a = Link::open(0xA11CE).unwrap();
        let mut b = Link::open(0xB0B).unwrap();
        let mut data = SlotData { label: label("drums", 7, 0xA11CE), ..Default::default() };
        data.bars[0] = Occupancy { song_bar: 3, steps: [9; STEPS_PER_BAR], ..Default::default() };
        // A pitched lane's line and the form position cross too (D46).
        data.bars[0].keys[3] = 40;
        data.bars[0].form_bar = 17;
        data.bars[0].flags = mesh_vst_core::splice::PITCHED | mesh_vst_core::splice::FORMED;
        // And the seeds its form is built from (D48).
        let spec = mesh_vst_core::macros::FormSettings::seeded(12).spec(true, 0.25, 1.0);
        data.bars[0].set_form(&spec);
        a.publish(&data);
        b.publish(&SlotData { label: label("bass", 8, 0xB0B), ..Default::default() });
        let seen = b.read(0xA11CE).expect("b reads a");
        assert_eq!(seen.bars[0].song_bar, 3);
        assert_eq!((seen.bars[0].keys[3], seen.bars[0].form_bar, seen.bars[0].flags), (40, 17, 3));
        assert_eq!(seen.bars[0].form_spec(), spec);

        assert!(b.peers().iter().any(|p| p.id == 0xA11CE && p.label.starts_with("drums 0007")));
        assert!(!b.peers().iter().any(|p| p.id == 0xB0B), "not itself");
        a.release();
        assert!(b.read(0xA11CE).is_none(), "a left");
    }

    #[test]
    fn the_sidechain_repeats_its_last_bar() {
        let mut sc = SidechainOcc::default();
        for s in 0..32 {
            let tick = s as f64 * mesh_vst_core::TICKS_PER_STEP as f64;
            sc.feed(tick, if s % 4 == 0 { 1.0 } else { 0.05 });
        }
        let bars = sc.bars(2);
        assert_eq!(bars[0].song_bar, 2);
        assert!(bars[0].steps[0] == 255 && bars[0].steps[1] < 20);
    }
}
