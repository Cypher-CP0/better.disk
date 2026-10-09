//! Deterministic synthetic levels for the shell spike.
//!
//! The algorithm is identical to `app/src/data/synthetic.ts` (same
//! generator, same call order, same arithmetic in IEEE doubles), so the
//! native shell and the browser preview show the same data. Date buckets use
//! the shared `bd_core::age` rules.

use bd_core::age::{AgeBucket, bucket_for};
use bd_core::time::{TICKS_PER_DAY, Ticks};
use serde::{Deserialize, Serialize};

/// mulberry32, matching the TypeScript implementation bit for bit.
pub struct Mulberry32(u32);

impl Mulberry32 {
    pub fn new(seed: u32) -> Mulberry32 {
        Mulberry32(seed)
    }

    pub fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.next() * n as f64).floor() as usize).min(n - 1)
    }
}

const WORDS: [&str; 16] = [
    "report", "invoice", "holiday", "backup", "photo", "project", "notes", "draft", "final",
    "budget", "video", "track", "scan", "export", "archive", "data",
];

const EXTS: [(&str, &str); 12] = [
    ("pdf", "Documents"),
    ("docx", "Documents"),
    ("txt", "Documents"),
    ("mp4", "Videos"),
    ("mkv", "Videos"),
    ("zip", "Archives"),
    ("7z", "Archives"),
    ("mp3", "Music"),
    ("jpg", "Images"),
    ("png", "Images"),
    ("dll", "Other"),
    ("bin", "Other"),
];

/// The largest level the spike will generate.
pub const MAX_COUNT: u32 = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelSpec {
    pub seed: u32,
    pub count: u32,
    pub even: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    SizeDesc,
    NameAsc,
    ModifiedDesc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    File,
    Folder,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub bytes: u64,
    pub files: u32,
    pub bucket: AgeBucket,
    pub file_type: &'static str,
    pub modified_days_ago: Option<u32>,
}

pub fn generate(spec: LevelSpec, now: Ticks) -> Vec<Item> {
    let mut rng = Mulberry32::new(spec.seed);
    let count = spec.count.min(MAX_COUNT);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let folder = rng.next() < 0.3;
        let word = WORDS[rng.pick(WORDS.len())];
        let (ext, file_type) = EXTS[rng.pick(EXTS.len())];
        let exponent = rng.pick(if folder { 36 } else { 31 }) as i32;
        let zero = !folder && rng.next() < 0.03;
        let bytes = if zero {
            0
        } else {
            let e = if spec.even { 30 } else { exponent };
            ((1.0 + rng.next()) * 2f64.powi(e)).floor() as u64
        };
        let files = if folder { 1 + rng.pick(5000) as u32 } else { 1 };
        let unknown = rng.next() < 0.04;
        let r = rng.next();
        let days = (!unknown).then(|| (r * r * 2000.0).floor() as u32);
        let bucket = bucket_for(
            now,
            days.and_then(|d| now.checked_add_ticks(-(i64::from(d) * TICKS_PER_DAY))),
        );
        out.push(Item {
            id: i,
            name: if folder {
                format!("{word} {i}")
            } else {
                format!("{word}-{i}.{ext}")
            },
            kind: if folder { Kind::Folder } else { Kind::File },
            bytes,
            files,
            bucket,
            file_type: if folder { "Folder" } else { file_type },
            modified_days_ago: days,
        });
    }
    out
}

/// UTF-16 code-unit order, matching the frontend and Windows ordinal order.
fn ordinal(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

pub fn sort(items: &mut [Item], key: SortKey) {
    match key {
        SortKey::SizeDesc => items.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| ordinal(&a.name, &b.name))
                .then(a.id.cmp(&b.id))
        }),
        SortKey::NameAsc => items.sort_by(|a, b| ordinal(&a.name, &b.name).then(a.id.cmp(&b.id))),
        SortKey::ModifiedDesc => items.sort_by(|a, b| {
            let da = a.modified_days_ago.unwrap_or(u32::MAX);
            let db = b.modified_days_ago.unwrap_or(u32::MAX);
            da.cmp(&db)
                .then_with(|| ordinal(&a.name, &b.name))
                .then(a.id.cmp(&b.id))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_matches_typescript_reference() {
        let mut r = Mulberry32::new(42);
        let first: Vec<u64> = (0..3)
            .map(|_| (r.next() * 4_294_967_296.0) as u64)
            .collect();
        assert_eq!(first, vec![2_581_720_956, 1_925_393_290, 3_661_312_704]);
    }

    #[test]
    fn deterministic_with_inclusive_buckets() {
        let now = Ticks::now();
        let spec = LevelSpec {
            seed: 7,
            count: 500,
            even: false,
        };
        let a = generate(spec, now);
        let b = generate(spec, now);
        assert_eq!(a.len(), 500);
        assert!(
            a.iter()
                .zip(&b)
                .all(|(x, y)| x.name == y.name && x.bytes == y.bytes)
        );
        for item in &a {
            let expected = match item.modified_days_ago {
                None => AgeBucket::Unknown,
                Some(0..=7) => AgeBucket::Within7Days,
                Some(8..=30) => AgeBucket::Days8To30,
                Some(31..=90) => AgeBucket::Days31To90,
                Some(91..=365) => AgeBucket::Days91To365,
                Some(_) => AgeBucket::OverOneYear,
            };
            assert_eq!(item.bucket, expected, "{}", item.name);
        }
    }

    #[test]
    fn sorts() {
        let mut items = generate(
            LevelSpec {
                seed: 3,
                count: 200,
                even: false,
            },
            Ticks::now(),
        );
        sort(&mut items, SortKey::SizeDesc);
        assert!(items.windows(2).all(|w| w[0].bytes >= w[1].bytes));
        sort(&mut items, SortKey::ModifiedDesc);
        assert!(items.last().unwrap().modified_days_ago.is_none());
    }
}
