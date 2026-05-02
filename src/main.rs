mod engine;
mod error;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use engine::KvStore;
use error::KVError;

/// kvs — a persistent log-structured key-value store
#[derive(Parser)]
#[command(
    name = "kvs",
    version,
    about = "A persistent log-structured key-value store",
    long_about = "kvs stores key-value pairs on disk using an append-only log\n\
                  with automatic background compaction.\n\n\
                  Data is stored in the directory specified by --dir\n\
                  (defaults to ./kvs-data)."
)]
struct Cli {
    /// Directory where the database files are stored
    #[arg(long, default_value = "kvs-data", global = true)]
    dir: PathBuf,

    /// Increase log verbosity (-v info, -vv debug)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set KEY to VALUE (insert or overwrite)
    Set { key: String, value: String },
    /// Get the value for KEY
    Get { key: String },
    /// Delete KEY from the store  (aliases: del, rm)
    #[command(alias = "del", alias = "rm")]
    Delete { key: String },
    /// List keys and values, optionally filtered by PREFIX
    Scan {
        #[arg(default_value = "")]
        prefix: String,
        /// Print keys only, omit values
        #[arg(long)]
        keys_only: bool,
    },
    /// Force manual compaction of log files
    Compact,
    /// Show store statistics
    Stats,
}

fn main() {
    let cli = Cli::parse();

    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    env_logger::Builder::new()
        .filter_level(log_level.parse().unwrap())
        .init();

    if let Err(e) = run(cli) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> error::Result<()> {
    let mut store = KvStore::open(&cli.dir)?;

    match cli.command {
        Commands::Set { key, value } => {
            store.set(key.clone(), value.clone())?;
            println!("OK  {} = {}", key, value);
        }

        Commands::Get { key } => match store.get(&key) {
            Ok(value) => println!("{}", value),
            Err(KVError::KeyNotFound(_)) => {
                eprintln!("(nil)  key not found: {}", key);
                process::exit(2);
            }
            Err(e) => return Err(e),
        },

        Commands::Delete { key } => match store.delete(&key) {
            Ok(()) => println!("Deleted: {}", key),
            Err(KVError::KeyNotFound(_)) => {
                eprintln!("(nil)  key not found: {}", key);
                process::exit(2);
            }
            Err(e) => return Err(e),
        },

        Commands::Scan { prefix, keys_only } => {
            let keys = store.prefix_scan(&prefix);
            if keys.is_empty() {
                if prefix.is_empty() {
                    println!("(empty store)");
                } else {
                    println!("(no keys matching prefix {:?})", prefix);
                }
                return Ok(());
            }
            println!("{} result(s):", keys.len());
            println!("{:-<50}", "");
            if keys_only {
                for k in &keys { println!("{}", k); }
            } else {
                for k in &keys {
                    match store.get(k) {
                        Ok(v)  => println!("  {:<30} = {}", k, v),
                        Err(_) => println!("  {:<30} = <error reading value>", k),
                    }
                }
            }
            println!("{:-<50}", "");
        }

        Commands::Compact => {
            println!("Running compaction…");
            store.compact()?;
            println!("Compaction complete.");
        }

        Commands::Stats => {
            let live = store.live_key_count();
            let dir = cli.dir.canonicalize().unwrap_or(cli.dir);
            let disk_bytes: u64 = std::fs::read_dir(&dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_name().to_string_lossy().starts_with("log-"))
                        .filter_map(|e| e.metadata().ok())
                        .map(|m| m.len())
                        .sum()
                })
                .unwrap_or(0);
            println!("Store statistics");
            println!("{:=<40}", "");
            println!("  Directory  : {}", dir.display());
            println!("  Live keys  : {}", live);
            println!("  Disk usage : {} bytes", disk_bytes);
        }
    }

    Ok(())
}