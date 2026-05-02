# ByteStore

A persistent, file-backed key-value database built in Rust — featuring a log-structured storage engine with automatic compaction and a clean CLI interface.

```
$ bytestore set user:1 '{"name":"Alice","role":"admin"}'
OK  user:1 = {"name":"Alice","role":"admin"}

$ bytestore scan user:
2 result(s):
--------------------------------------------------
  user:1                         = {"name":"Alice","role":"admin"}
  user:2                         = {"name":"Bob","role":"viewer"}
--------------------------------------------------
```

---

## Why I built this

Most databases feel like black boxes. I wanted to understand what actually happens when you call `set` — how bytes land on disk, how reads find them again after a restart, and how compaction reclaims space without blocking writers. Building ByteStore from scratch gave me a clear mental model of the internals behind systems like Bitcask and LevelDB.

---

## Features

- **Persistent** — data survives process restarts; the log is replayed on startup to rebuild the index
- **Log-structured writes** — every write is an append, no random I/O, no in-place mutation
- **Automatic compaction** — stale entries (overwrites and deletes) are garbage-collected once a threshold is reached; disk usage shrinks without any manual intervention
- **Crash-safe replay** — the engine scans all segment files on open and reconstructs the in-memory index from scratch
- **Prefix scan** — efficiently list all keys sharing a common prefix
- **Typed errors** — every failure path returns a structured `KVError` rather than a raw string
- **Verbosity levels** — pass `-v` (info) or `-vv` (debug) to trace compaction decisions and segment lifecycle

---

## Architecture

```
kvs-data/
  log-00000000000000000000.dat   ← segment 0  (append-only)
  log-00000000000000000002.dat   ← segment 2  (after first compaction)
  log-00000000000000000003.dat   ← segment 3  (active write segment)
```

Every `set` and `delete` appends a JSON record to the active segment:

```json
{"cmd":"Set","key":"lang","value":"Rust"}
{"cmd":"Delete","key":"lang"}
```

An **in-memory hash-map** maps each live key to a `(segment_id, byte_offset, byte_len)` triple. Reads seek directly to that offset — O(1) regardless of log size.

### Compaction

When the count of stale entries (overwrites + deletes) crosses `COMPACTION_THRESHOLD` (default 1 000):

1. All live entries are copied into a fresh compaction segment
2. A new empty write segment is opened
3. All older segments are deleted from disk

The whole process is synchronous and runs in the same thread. No background threads, no locks needed.

```
Before compaction          After compaction
──────────────────         ────────────────
seg-0: Set a=1             seg-2: Set a=3   ← compacted live data
seg-0: Set b=2             seg-3: (empty)   ← new write target
seg-0: Set a=2    ──────►
seg-0: Delete b
seg-0: Set a=3
  (4 stale records)
```

### File layout per segment

| Field    | Format                    |
|----------|---------------------------|
| Command  | JSON object, one per line |
| Newline  | `\n` separator            |
| Encoding | UTF-8                     |

---

## Getting started

**Prerequisites:** Rust 1.75+ ([install](https://rustup.rs))

```bash
git clone https://github.com/YOUR_USERNAME/ByteStore.git
cd ByteStore
cargo build --release
```

The binary lands at `target/release/kvs`.

---

## Usage

```
kvs [OPTIONS] <COMMAND>

Options:
  --dir <DIR>    Data directory  [default: ./kvs-data]
  -v             Verbose  (-vv for debug)

Commands:
  set <KEY> <VALUE>    Insert or overwrite a key
  get <KEY>            Retrieve a value
  delete <KEY>         Remove a key  (aliases: del, rm)
  scan [PREFIX]        List matching keys and values
    --keys-only        Print keys only
  compact              Force a compaction run
  stats                Live key count and disk usage
  help                 Show this help
```

### Examples

```bash
# Basic CRUD
kvs set config.debug true
kvs get config.debug        # → true
kvs delete config.debug

# Namespace-style key prefixes
kvs set user:1 Alice
kvs set user:2 Bob
kvs set session:abc token123

kvs scan user:              # → user:1, user:2
kvs scan --keys-only        # → all keys, no values

# Inspect the store
kvs stats
kvs --dir /tmp/mydb stats

# Verbose compaction trace
kvs -vv compact
```

---

## Project structure

```
src/
  main.rs      — CLI definition (clap) and command dispatch
  engine.rs    — KvStore, SegmentWriter, SegmentReader, compaction logic
  error.rs     — KVError enum (thiserror)
```

---

## Crates used

| Crate | Purpose |
|---|---|
| [`clap`](https://docs.rs/clap) | Derive-based CLI argument parsing |
| [`serde`](https://docs.rs/serde) + [`serde_json`](https://docs.rs/serde_json) | Command serialisation to/from JSON |
| [`thiserror`](https://docs.rs/thiserror) | Ergonomic typed error enum |
| [`log`](https://docs.rs/log) + [`env_logger`](https://docs.rs/env_logger) | Structured logging with runtime verbosity |

---

## License

MIT
