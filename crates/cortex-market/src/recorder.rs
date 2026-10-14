use flate2::{Compression, write::GzEncoder};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, mpsc},
};

pub struct Recorder {
    tx: mpsc::SyncSender<Value>,
    dropped: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
}
impl Recorder {
    pub fn new(folder: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let folder = folder.to_path_buf();
        let (tx, rx) = mpsc::sync_channel::<Value>(2048);
        let dropped = Arc::new(AtomicU64::new(0));
        let written = Arc::new(AtomicU64::new(0));
        let gaps = dropped.clone();
        let count = written.clone();
        std::thread::Builder::new()
            .name("market-recorder".into())
            .spawn(move || {
                let mut previous_drop = 0;
                let mut last_prune = std::time::Instant::now();
                for mut event in rx {
                    let total = gaps.load(Ordering::Relaxed);
                    event["dropped_before"] = serde_json::json!(total - previous_drop);
                    previous_drop = total;
                    if write(&folder, &event).is_ok() {
                        count.fetch_add(1, Ordering::Relaxed);
                    } else {
                        gaps.fetch_add(1, Ordering::Relaxed);
                    }
                    if last_prune.elapsed().as_secs() > 60 {
                        prune(&folder);
                        last_prune = std::time::Instant::now();
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            dropped,
            written,
        })
    }
    pub fn record(&self, event: Value) {
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn counts(&self) -> (u64, u64) {
        (
            self.written.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
        )
    }
}
fn write(folder: &Path, event: &Value) -> Result<(), String> {
    let file = folder.join(format!("{}.jsonl.gz", crate::clock::now_ms() / 86_400_000));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|e| e.to_string())?;
    let mut compressed = GzEncoder::new(file, Compression::fast());
    serde_json::to_writer(&mut compressed, event).map_err(|e| e.to_string())?;
    compressed.write_all(b"\n").map_err(|e| e.to_string())?;
    compressed.finish().map_err(|e| e.to_string())?;
    Ok(())
}
fn prune(folder: &Path) {
    let oldest = crate::clock::now_ms() / 86_400_000 - 14;
    let mut files: Vec<(PathBuf, i64, u64)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let day = name.to_str()?.strip_suffix(".jsonl.gz")?.parse().ok()?;
            Some((e.path(), day, e.metadata().ok()?.len()))
        })
        .collect();
    files.sort_by_key(|(_, day, _)| *day);
    let mut size: u64 = files.iter().map(|(_, _, size)| size).sum();
    for (path, day, length) in files {
        if day < oldest || (size > 1_073_741_824 && day < crate::clock::now_ms() / 86_400_000) {
            if std::fs::remove_file(path).is_ok() {
                size = size.saturating_sub(length);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concatenated_compressed_records_are_replayable() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &serde_json::json!({"id":1})).unwrap();
        write(dir.path(), &serde_json::json!({"id":2})).unwrap();
        let file = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut decoder = flate2::read::MultiGzDecoder::new(std::fs::File::open(file).unwrap());
        let mut text = String::new();
        std::io::Read::read_to_string(&mut decoder, &mut text).unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
