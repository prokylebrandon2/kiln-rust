//! Command-line definitions and command handlers.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use kiln::bench::{self, BenchConfig};
use kiln::codec::CodecSpec;
use kiln::format::index::EntryKind;
use kiln::format::{DEFAULT_BLOCK_LOG2, MAX_BLOCK_LOG2, MIN_BLOCK_LOG2};
use kiln::integrity::to_hex;
use kiln::parallel::resolve_threads;
use kiln::util::{format_size, format_utc, mib_per_sec, parse_size, ratio};
use kiln::{Archive, ExtractOptions, PackOptions};

/// Kiln: a fast multicore archiver (Rust rewrite, phase 1 foundation).
#[derive(Parser, Debug)]
#[command(name = "kiln", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create an archive from files and directories
    Pack(PackArgs),
    /// Extract an archive (or just some members of it)
    Unpack(UnpackArgs),
    /// Show an archive's contents (instant: reads only the index)
    List(ListArgs),
    /// Verify every checksum without extracting
    Test(TestArgs),
    /// Measure codecs and block sizes on your own data
    Bench(BenchArgs),
}

#[derive(Args, Debug)]
struct PackArgs {
    /// Archive to create
    archive: PathBuf,
    /// Files and directories to add
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Compression: store, lz4, zstd[:-7..22], lzma[:0..9]
    #[arg(short, long, default_value = "zstd:3")]
    codec: String,
    /// Block size, a power of two from 4KiB to 128MiB (bigger = better ratio, more memory)
    #[arg(short, long, default_value = "4MiB", value_parser = parse_block_size)]
    block_size: u8,
    /// Worker threads (0 = all cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
    /// Overwrite the archive if it exists
    #[arg(short, long)]
    force: bool,
    /// Print nothing except errors and warnings
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args, Debug)]
struct UnpackArgs {
    /// Archive to read
    archive: PathBuf,
    /// Extract only these members (a file, or a directory and everything below it)
    members: Vec<String>,
    /// Destination directory
    #[arg(short, long, default_value = ".")]
    output: PathBuf,
    /// Worker threads (0 = all cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
    /// Overwrite existing files
    #[arg(short, long)]
    force: bool,
    /// Print nothing except errors
    #[arg(short, long)]
    quiet: bool,
}

#[derive(Args, Debug)]
struct ListArgs {
    /// Archive to read
    archive: PathBuf,
    /// Show size, modification time, permissions and hash for every entry
    #[arg(short, long)]
    long: bool,
}

#[derive(Args, Debug)]
struct TestArgs {
    /// Archive to verify
    archive: PathBuf,
    /// Worker threads (0 = all cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
}

#[derive(Args, Debug)]
struct BenchArgs {
    /// Files and directories to compress
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Codecs to compare (repeat the flag); default: lz4, zstd:3, zstd:19, lzma:6
    #[arg(short, long = "codec")]
    codecs: Vec<String>,
    /// Block sizes to compare (repeat the flag); default: 4MiB
    #[arg(short, long = "block-size", value_parser = parse_block_size)]
    block_sizes: Vec<u8>,
    /// Worker threads (0 = all cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
}

fn parse_block_size(text: &str) -> Result<u8, String> {
    let bytes = parse_size(text)?;
    if !bytes.is_power_of_two() {
        return Err(format!("{text:?} is not a power of two"));
    }
    let log2 = bytes.trailing_zeros() as u8;
    if !(MIN_BLOCK_LOG2..=MAX_BLOCK_LOG2).contains(&log2) {
        return Err("block size must be between 4KiB and 128MiB".into());
    }
    Ok(log2)
}

/// Run the parsed command line.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Pack(a) => pack(a),
        Command::Unpack(a) => unpack(a),
        Command::List(a) => list(a),
        Command::Test(a) => test(a),
        Command::Bench(a) => benchmark(a),
    }
}

fn pack(a: PackArgs) -> Result<()> {
    let options = PackOptions {
        codec: CodecSpec::parse(&a.codec)?,
        block_log2: a.block_size,
        threads: a.threads,
        overwrite: a.force,
    };
    let started = Instant::now();
    let stats = kiln::pack(&a.archive, &a.inputs, &options)?;
    let secs = started.elapsed().as_secs_f64();
    for s in &stats.skipped {
        eprintln!("warning: skipped {} ({})", s.path.display(), s.reason);
    }
    for p in &stats.grew {
        eprintln!(
            "warning: {} grew while being read; only its original size was archived",
            p.display()
        );
    }
    if !a.quiet {
        println!(
            "packed {} files, {} dirs: {} -> {} ({:.2}x) in {secs:.2}s, {:.0} MiB/s  [{}, {} blocks, {} threads]",
            stats.files,
            stats.dirs,
            format_size(stats.raw_bytes),
            format_size(stats.archive_bytes),
            ratio(stats.raw_bytes, stats.archive_bytes),
            mib_per_sec(stats.raw_bytes, secs),
            options.codec,
            format_size(1 << options.block_log2),
            resolve_threads(a.threads),
        );
    }
    Ok(())
}

fn unpack(a: UnpackArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    let options = ExtractOptions {
        dest: a.output,
        overwrite: a.force,
        threads: a.threads,
        members: a.members,
    };
    let started = Instant::now();
    let stats = kiln::extract(&archive, &options)?;
    let secs = started.elapsed().as_secs_f64();
    if stats.metadata_errors > 0 {
        eprintln!(
            "warning: could not restore {} timestamps/permissions",
            stats.metadata_errors
        );
    }
    if !a.quiet {
        println!(
            "extracted {} files, {} dirs ({}) in {secs:.2}s, {:.0} MiB/s  [{} of {} blocks decoded]",
            stats.files,
            stats.dirs,
            format_size(stats.bytes),
            mib_per_sec(stats.bytes, secs),
            stats.blocks,
            archive.block_count(),
        );
    }
    Ok(())
}

fn list(a: ListArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    for e in archive.entries() {
        let suffix = if e.kind == EntryKind::Dir { "/" } else { "" };
        if a.long {
            println!(
                "{:>12}  {}  {:04o}  {}  {}{suffix}",
                e.size,
                format_utc(e.mtime_secs),
                e.mode & 0o7777,
                &to_hex(&e.hash)[..12],
                e.path
            );
        } else {
            println!("{}{suffix}", e.path);
        }
    }
    let files = archive
        .entries()
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .count();
    eprintln!(
        "{} files, {} dirs, {} -> {} ({:.2}x), {} blocks of {}, default codec {}",
        files,
        archive.entries().len() - files,
        format_size(archive.total_raw()),
        format_size(archive.file_len()),
        ratio(archive.total_raw(), archive.file_len()),
        archive.block_count(),
        format_size(archive.header().block_size()),
        archive.header().default_method.name(),
    );
    Ok(())
}

fn test(a: TestArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    let started = Instant::now();
    let stats = kiln::verify(&archive, a.threads)?;
    let secs = started.elapsed().as_secs_f64();
    println!(
        "OK: {} files, {} dirs, {} verified in {secs:.2}s ({:.0} MiB/s)",
        stats.files,
        stats.dirs,
        format_size(stats.bytes),
        mib_per_sec(stats.bytes, secs)
    );
    Ok(())
}

fn benchmark(a: BenchArgs) -> Result<()> {
    let codec_names = if a.codecs.is_empty() {
        ["lz4", "zstd:3", "zstd:19", "lzma:6"]
            .map(String::from)
            .to_vec()
    } else {
        a.codecs
    };
    let mut codecs = Vec::new();
    for name in &codec_names {
        codecs.push(CodecSpec::parse(name)?);
    }
    let block_log2s = if a.block_sizes.is_empty() {
        vec![DEFAULT_BLOCK_LOG2]
    } else {
        a.block_sizes
    };
    if codecs.is_empty() || block_log2s.is_empty() {
        bail!("nothing to benchmark");
    }
    println!(
        "{} worker threads; timings are wall-clock and include page-cache effects",
        resolve_threads(a.threads)
    );
    println!(
        "{:<10} {:>8} {:>12} {:>8} {:>10} {:>12} {:>10}",
        "codec", "block", "size", "ratio", "pack MiB/s", "verify MiB/s", "pack s"
    );
    let config = BenchConfig {
        inputs: a.inputs,
        codecs,
        block_log2s,
        threads: a.threads,
    };
    bench::run(&config, |r| {
        println!(
            "{:<10} {:>8} {:>12} {:>8.3} {:>10.0} {:>12.0} {:>10.2}",
            r.codec.to_string(),
            format_size(1 << r.block_log2),
            r.archive_bytes,
            ratio(r.raw_bytes, r.archive_bytes),
            mib_per_sec(r.raw_bytes, r.pack_secs),
            mib_per_sec(r.raw_bytes, r.verify_secs),
            r.pack_secs,
        );
    })?;
    Ok(())
}
