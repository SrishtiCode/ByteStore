use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use log::{debug, info};
use serde::{Deserialize, Serialize};

use crate::error::{KVError, Result};

const COMPACTION_THRESHOLD: u64 = 1_000;

// ── On-disk record ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd")]
pub enum Command {
    Set { key: String, value: String },
    Delete { key: String },
}

// ── In-memory index entry ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct EntryPointer {
    seg_id: u64,
    offset: u64,
    len: u64,
}

// ── Engine ─────────────────────────────────────────────────────────────────────

pub struct KvStore {
    dir: PathBuf,
    readers: BTreeMap<u64, SegmentReader>,
    writer: SegmentWriter,
    current_seg: u64,
    index: HashMap<String, EntryPointer>,
    stale: u64,
}

impl KvStore {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;

        let seg_ids = sorted_seg_ids(&dir)?;
        let current_seg = seg_ids.last().copied().unwrap_or(0);

        let mut readers: BTreeMap<u64, SegmentReader> = BTreeMap::new();
        let mut index: HashMap<String, EntryPointer> = HashMap::new();
        let mut stale: u64 = 0;

        for &sid in &seg_ids {
            let path = segment_path(&dir, sid);
            let mut reader = SegmentReader::new(&path)?;
            let entries = reader.replay(sid)?;
            for (key, ptr, is_delete) in entries {
                if is_delete {
                    if index.remove(&key).is_some() {
                        stale += 1;
                    }
                    stale += 1;
                } else if index.insert(key, ptr).is_some() {
                    stale += 1;
                }
            }
            readers.insert(sid, reader);
        }

        let write_path = segment_path(&dir, current_seg);
        let writer = SegmentWriter::new(&write_path)?;
        readers
            .entry(current_seg)
            .or_insert_with(|| SegmentReader::new(&write_path).expect("open reader"));

        Ok(KvStore { dir, readers, writer, current_seg, index, stale })
    }

    // ── Public API ──────────────────────────────────────────────────────────────

    pub fn set(&mut self, key: String, value: String) -> Result<()> {
        let cmd = Command::Set { key: key.clone(), value };
        let ptr = self.append_command(&cmd)?;
        if self.index.insert(key, ptr).is_some() {
            self.stale += 1;
        }
        self.maybe_compact()
    }

    pub fn get(&mut self, key: &str) -> Result<String> {
        let ptr = self
            .index
            .get(key)
            .cloned()
            .ok_or_else(|| KVError::KeyNotFound(key.to_owned()))?;

        let reader = self
            .readers
            .get_mut(&ptr.seg_id)
            .ok_or_else(|| KVError::CorruptLog {
                offset: ptr.offset,
                reason: format!("missing segment {}", ptr.seg_id),
            })?;

        match reader.read_at(ptr.offset, ptr.len)? {
            Command::Set { value, .. } => Ok(value),
            Command::Delete { key } => Err(KVError::KeyNotFound(key)),
        }
    }

    pub fn delete(&mut self, key: &str) -> Result<()> {
        if !self.index.contains_key(key) {
            return Err(KVError::KeyNotFound(key.to_owned()));
        }
        let cmd = Command::Delete { key: key.to_owned() };
        self.append_command(&cmd)?;
        self.index.remove(key);
        self.stale += 1;
        self.maybe_compact()
    }

    /// Return sorted keys whose names start with `prefix` (empty = all keys).
    pub fn prefix_scan(&self, prefix: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .index
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    // ── Internals ───────────────────────────────────────────────────────────────

    fn append_command(&mut self, cmd: &Command) -> Result<EntryPointer> {
        let (offset, len) = self.writer.append(cmd)?;
        let path = segment_path(&self.dir, self.current_seg);
        self.readers
            .entry(self.current_seg)
            .or_insert_with(|| SegmentReader::new(&path).expect("reopen reader"));
        Ok(EntryPointer { seg_id: self.current_seg, offset, len })
    }

    fn maybe_compact(&mut self) -> Result<()> {
        if self.stale >= COMPACTION_THRESHOLD {
            self.compact()?;
        }
        Ok(())
    }

    pub fn compact(&mut self) -> Result<()> {
        let compact_id = self.current_seg + 1;
        let new_id = self.current_seg + 2;
        info!("Compacting: {} stale entries → segment {}", self.stale, compact_id);

        let compact_path = segment_path(&self.dir, compact_id);
        let mut compact_writer = SegmentWriter::new(&compact_path)?;
        let mut new_index: HashMap<String, EntryPointer> = HashMap::new();

        for (key, ptr) in &self.index {
            let reader = self
                .readers
                .get_mut(&ptr.seg_id)
                .ok_or_else(|| KVError::CorruptLog {
                    offset: ptr.offset,
                    reason: "segment missing during compaction".into(),
                })?;
            let cmd = reader.read_at(ptr.offset, ptr.len)?;
            let (offset, len) = compact_writer.append(&cmd)?;
            new_index.insert(key.clone(), EntryPointer { seg_id: compact_id, offset, len });
        }
        compact_writer.flush()?;
        debug!("Compaction written: {} entries", new_index.len());

        let new_path = segment_path(&self.dir, new_id);
        let new_writer = SegmentWriter::new(&new_path)?;

        let old_ids: Vec<u64> = self
            .readers
            .keys()
            .filter(|&&s| s < compact_id)
            .copied()
            .collect();
        for sid in old_ids {
            self.readers.remove(&sid);
            let _ = fs::remove_file(segment_path(&self.dir, sid));
        }

        self.readers.insert(compact_id, SegmentReader::new(&compact_path)?);
        self.readers.insert(new_id, SegmentReader::new(&new_path)?);
        self.writer = new_writer;
        self.current_seg = new_id;
        self.index = new_index;
        self.stale = 0;

        info!("Compaction complete. Active segment: {}", new_id);
        Ok(())
    }

    pub fn live_key_count(&self) -> usize {
        self.index.len()
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn segment_path(dir: &Path, sid: u64) -> PathBuf {
    dir.join(format!("log-{:020}.dat", sid))
}

fn sorted_seg_ids(dir: &Path) -> Result<Vec<u64>> {
    let mut ids: Vec<u64> = fs::read_dir(dir)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|name| {
            name.strip_prefix("log-")
                .and_then(|s| s.strip_suffix(".dat"))
                .and_then(|s| s.parse::<u64>().ok())
        })
        .collect();
    ids.sort_unstable();
    Ok(ids)
}

// ── SegmentWriter ──────────────────────────────────────────────────────────────

struct SegmentWriter {
    writer: BufWriter<File>,
    pos: u64,
}

impl SegmentWriter {
    fn new(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let pos = file.metadata()?.len();
        Ok(SegmentWriter { writer: BufWriter::new(file), pos })
    }

    fn append(&mut self, cmd: &Command) -> Result<(u64, u64)> {
        let mut bytes = serde_json::to_vec(cmd)?;
        bytes.push(b'\n');
        let start = self.pos;
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        self.pos += bytes.len() as u64;
        Ok((start, bytes.len() as u64))
    }

    fn flush(&mut self) -> Result<()> {
        self.writer.flush().map_err(KVError::Io)
    }
}

// ── SegmentReader ──────────────────────────────────────────────────────────────

struct SegmentReader {
    reader: BufReader<File>,
}

impl SegmentReader {
    fn new(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .open(path)?;
        Ok(SegmentReader { reader: BufReader::new(file) })
    }

    fn read_at(&mut self, offset: u64, len: u64) -> Result<Command> {
        self.reader.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        self.reader.read_exact(&mut buf)?;
        let end = buf
            .iter()
            .rposition(|&b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(0);
        Ok(serde_json::from_slice(&buf[..end])?)
    }

    fn replay(&mut self, seg_id: u64) -> Result<Vec<(String, EntryPointer, bool)>> {
        use std::io::BufRead;
        self.reader.seek(SeekFrom::Start(0))?;
        let mut entries = Vec::new();
        let mut offset: u64 = 0;
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 { break; }
            let len = n as u64;
            let slice = line.trim_end();
            if slice.is_empty() { offset += len; continue; }
            let cmd: Command = serde_json::from_str(slice).map_err(|e| KVError::CorruptLog {
                offset,
                reason: e.to_string(),
            })?;
            let (key, is_delete) = match &cmd {
                Command::Set { key, .. } => (key.clone(), false),
                Command::Delete { key }  => (key.clone(), true),
            };
            entries.push((key, EntryPointer { seg_id, offset, len }, is_delete));
            offset += len;
        }
        Ok(entries)
    }
}