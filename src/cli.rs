//! Command-line definitions and command handlers.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use kiln::bench::{self, BenchConfig};
use kiln::codec::CodecSpec;
use kiln::format::index::EntryKind;
use kiln::format::{DEFAULT_BLOCK_LOG2, FOOTER_LEN, MAX_BLOCK_LOG2, MIN_BLOCK_LOG2};
use kiln::integrity::to_hex;
use kiln::parallel::resolve_threads;
use kiln::util::{format_size, format_utc, mib_per_sec, parse_size, ratio};
use kiln::{Archive, ExtractOptions, PackOptions, Profile};

/// Kiln: a fast multicore archiver with its own `.kiln` container format (Rust rewrite, phase 3).
///
/// Exit codes: 0 success, 1 other error, 2 usage error, 3 I/O error, 4 not a Kiln archive or
/// unsupported version, 5 corrupt archive or checksum mismatch, 6 request refused (output exists,
/// unsafe path, bad option, missing member), 7 completed with warnings.
#[derive(Parser, Debug)]
#[command(name = "kiln", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create an archive from files and directories
    #[command(visible_alias = "pack")]
    Create(PackArgs),
    /// Extract an archive (or just some members of it)
    #[command(visible_aliases = ["unpack", "x"])]
    Extract(UnpackArgs),
    /// Show an archive's contents (instant: reads only the index)
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Verify every checksum without extracting
    Test(TestArgs),
    /// Show archive-level information (format, codec, sizes, counts)
    Info(InfoArgs),
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
    /// Preset: fastest, balanced (default), maximum, store or adaptive
    #[arg(short, long, value_name = "NAME")]
    profile: Option<String>,
    /// Override the profile's codec: store, lz4, zstd[:-7..22], lzma[:0..9]
    #[arg(short, long)]
    codec: Option<String>,
    /// Override the profile's chunk size: a power of two from 4KiB to 128MiB
    #[arg(short, long, value_parser = parse_block_size)]
    block_size: Option<u8>,
    /// Sample every file and store incompressible ones separately (on by default for the
    /// `maximum` and `adaptive` profiles)
    #[arg(long, conflicts_with = "no_classify")]
    classify: bool,
    /// Never classify files, whatever the profile says
    #[arg(long)]
    no_classify: bool,
    /// Store files with identical content once (reads files that share a size an extra time)
    #[arg(long)]
    dedup: bool,
    /// Worker threads (0 = all cores)
    #[arg(short = 'j', long, default_value_t = 0)]
    threads: usize,
    /// Overwrite the archive if it exists
    #[arg(short, long)]
    force: bool,
    /// Print nothing except errors and warnings
    #[arg(short, long)]
    quiet: bool,
    /// Keep estimated peak memory under this size (e.g. 256MiB) by using fewer threads/blocks
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    memory_limit: Option<u64>,
    /// Always attempt compression, even on blocks that look incompressible (slow codecs only)
    #[arg(long)]
    no_precheck: bool,
    /// Show a progress line on stderr
    #[arg(long)]
    progress: bool,
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
    /// Keep estimated peak memory under this size (e.g. 256MiB) by using fewer threads/blocks
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    memory_limit: Option<u64>,
    /// Refuse to extract more than this many bytes (guards against decompression bombs)
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    max_size: Option<u64>,
    /// Show a progress line on stderr
    #[arg(long)]
    progress: bool,
}

#[derive(Args, Debug)]
struct ListArgs {
    /// Archive to read
    archive: PathBuf,
    /// Show size, class, codec, modification time, permissions and hash for every entry
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
    /// Keep estimated peak memory under this size (e.g. 256MiB) by using fewer threads/blocks
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    memory_limit: Option<u64>,
}

#[derive(Args, Debug)]
struct InfoArgs {
    /// Archive to inspect
    archive: PathBuf,
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

/// A throttled single-line progress display on stderr (only when `--progress` is given).
struct Meter {
    enabled: bool,
    last: Instant,
    drew: bool,
}

impl Meter {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last: Instant::now(),
            drew: false,
        }
    }

    fn update(&mut self, done: u64, total: u64) {
        if !self.enabled {
            return;
        }
        let finished = done >= total;
        if !finished && self.last.elapsed().as_millis() < 100 {
            return;
        }
        self.last = Instant::now();
        self.drew = true;
        let pct = if total == 0 {
            100.0
        } else {
            done as f64 * 100.0 / total as f64
        };
        eprint!(
            "\r{pct:5.1}%  {} / {}   ",
            format_size(done),
            format_size(total)
        );
    }

    /// Erase the progress line so the summary starts on a clean line.
    fn finish(&mut self) {
        if self.drew {
            eprint!("\r{}\r", " ".repeat(48));
        }
    }
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

/// How a successful run ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Everything requested was done.
    Clean,
    /// Done, but something was skipped or could not be restored (a warning was printed).
    Warnings,
}

/// Exit code for a failed run (see the `--help` text for the table).
pub fn exit_code(err: &anyhow::Error) -> u8 {
    use kiln::Error as E;
    match err.downcast_ref::<E>() {
        Some(E::Io { .. } | E::IoBare(_)) => 3,
        Some(E::NotAnArchive(_) | E::Unsupported(_)) => 4,
        Some(E::Corrupt(_) | E::Checksum(_)) => 5,
        Some(E::UnsafePath(_) | E::Invalid(_) | E::Exists(_)) => 6,
        // Codec failures while *reading* mean damaged data; FileChanged/Internal are other.
        Some(E::Codec { .. }) => 5,
        Some(E::FileChanged(_) | E::Internal(_)) | None => 1,
    }
}

/// Run the parsed command line.
pub fn run(cli: Cli) -> Result<Outcome> {
    match cli.command {
        Command::Create(a) => pack(a),
        Command::Extract(a) => unpack(a),
        Command::List(a) => list(a).map(|()| Outcome::Clean),
        Command::Test(a) => test(a).map(|()| Outcome::Clean),
        Command::Info(a) => info(a).map(|()| Outcome::Clean),
        Command::Bench(a) => benchmark(a).map(|()| Outcome::Clean),
    }
}

fn pack(a: PackArgs) -> Result<Outcome> {
    let profile = match (&a.profile, &a.codec) {
        (Some(name), _) => Profile::parse(name)?,
        (None, Some(_)) => Profile::Custom,
        (None, None) => Profile::Balanced,
    };
    let mut options = PackOptions::from_profile(profile);
    if let Some(codec) = &a.codec {
        options.codec = CodecSpec::parse(codec)?;
    }
    if let Some(log2) = a.block_size {
        options.block_log2 = log2;
    }
    if a.classify {
        options.classify = true;
    }
    if a.no_classify {
        options.classify = false;
    }
    if a.dedup {
        options.dedup = true;
    }
    options.threads = a.threads;
    options.overwrite = a.force;
    options.memory_limit = a.memory_limit;
    options.skip_incompressible = !a.no_precheck;
    let started = Instant::now();
    let mut meter = Meter::new(a.progress);
    let stats = kiln::pack_with_progress(&a.archive, &a.inputs, &options, &mut |d, t| {
        meter.update(d, t)
    });
    meter.finish();
    let stats = stats?;
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
    let warned = !stats.skipped.is_empty() || !stats.grew.is_empty();
    if !a.quiet {
        println!(
            "packed {} files, {} dirs: {} -> {} ({:.2}x) in {secs:.2}s, {:.0} MiB/s  [{} {}, {} chunks, {} threads{}]",
            stats.files,
            stats.dirs,
            format_size(stats.raw_bytes),
            format_size(stats.archive_bytes),
            ratio(stats.raw_bytes, stats.archive_bytes),
            mib_per_sec(stats.raw_bytes, secs),
            options.profile.name(),
            options.codec,
            format_size(1 << options.block_log2),
            resolve_threads(a.threads),
            if stats.classified_stored_files > 0 {
                format!(
                    ", {} files ({}) stored without compression",
                    stats.classified_stored_files,
                    format_size(stats.classified_stored_bytes)
                )
            } else {
                String::new()
            },
        );
    }
    Ok(if warned {
        Outcome::Warnings
    } else {
        Outcome::Clean
    })
}

fn unpack(a: UnpackArgs) -> Result<Outcome> {
    let archive = Archive::open(&a.archive)?;
    let options = ExtractOptions {
        dest: a.output,
        overwrite: a.force,
        threads: a.threads,
        members: a.members,
        memory_limit: a.memory_limit,
        max_bytes: a.max_size,
    };
    let started = Instant::now();
    let mut meter = Meter::new(a.progress);
    let stats = kiln::extract_with_progress(&archive, &options, &mut |d, t| meter.update(d, t));
    meter.finish();
    let stats = stats?;
    let secs = started.elapsed().as_secs_f64();
    if stats.metadata_errors > 0 {
        eprintln!(
            "warning: could not restore {} timestamps/permissions",
            stats.metadata_errors
        );
    }
    if !a.quiet {
        println!(
            "extracted {} files, {} dirs ({}) in {secs:.2}s, {:.0} MiB/s  [{} of {} chunks decoded]",
            stats.files,
            stats.dirs,
            format_size(stats.bytes),
            mib_per_sec(stats.bytes, secs),
            stats.blocks,
            archive.block_count(),
        );
    }
    Ok(if stats.metadata_errors > 0 {
        Outcome::Warnings
    } else {
        Outcome::Clean
    })
}

fn list(a: ListArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    // Stored in data order (grouped by compressibility); shown in name order.
    let mut order: Vec<usize> = (0..archive.entries().len()).collect();
    order.sort_by(|&x, &y| archive.entries()[x].path.cmp(&archive.entries()[y].path));
    for i in order {
        let e = &archive.entries()[i];
        let suffix = if e.kind == EntryKind::Dir { "/" } else { "" };
        if a.long {
            let method = archive.entry_method(i).map_or("-", |m| m.name());
            println!(
                "{:>12}  {:<10} {:<5} {}  {:04o}  {}  {}{suffix}",
                e.size,
                e.class.name(),
                method,
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
        "{} files, {} dirs, {} -> {} ({:.2}x), {} chunks of up to {}, profile {}",
        files,
        archive.entries().len() - files,
        format_size(archive.total_raw()),
        format_size(archive.file_len()),
        ratio(archive.total_raw(), archive.file_len()),
        archive.block_count(),
        format_size(archive.header().block_size()),
        archive.header().profile.name(),
    );
    Ok(())
}

fn info(a: InfoArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    let files = archive
        .entries()
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .count();
    let empty = archive
        .entries()
        .iter()
        .filter(|e| e.kind == EntryKind::File && e.size == 0)
        .count();
    let dirs = archive.entries().len() - files;
    let h = archive.header();
    println!("Archive:         {}", archive.path().display());
    println!(
        "Format:          .kiln container, version {}{}",
        kiln::format::FORMAT_VERSION,
        archive
            .index()
            .creator()
            .map_or(String::new(), |c| format!(" (written by {c})"))
    );
    println!("Profile:         {}", h.profile.name());
    println!("Chunk size:      up to {}", format_size(h.block_size()));
    println!("Files:           {files} ({empty} empty)");
    println!("Directories:     {dirs}");
    let blocks = archive.block_stats();
    let mut kinds = Vec::new();
    for (n, name) in [
        (blocks.zstd, "zstd"),
        (blocks.lz4, "lz4"),
        (blocks.lzma, "lzma"),
        (blocks.stored, "stored"),
    ] {
        if n > 0 {
            kinds.push(format!("{n} {name}"));
        }
    }
    println!("Chunks:          {} ({})", blocks.total, kinds.join(", "));
    let classes = archive.class_totals();
    if classes
        .iter()
        .any(|(c, _)| *c != kiln::classify::Class::Unknown)
    {
        let parts: Vec<String> = classes
            .iter()
            .map(|(c, t)| format!("{} {} ({})", t.files, c.name(), format_size(t.bytes)))
            .collect();
        println!("Content classes: {}", parts.join(", "));
    }
    let index_bytes = archive.file_len() - FOOTER_LEN as u64 - archive.index().data_end;
    println!(
        "Index:           {} ({:.2}% of the archive)",
        format_size(index_bytes),
        100.0 * index_bytes as f64 / archive.file_len() as f64
    );
    println!(
        "Uncompressed:    {} ({} bytes)",
        format_size(archive.total_raw()),
        archive.total_raw()
    );
    println!(
        "Archive size:    {} ({} bytes)",
        format_size(archive.file_len()),
        archive.file_len()
    );
    println!(
        "Ratio:           {:.3}x",
        ratio(archive.total_raw(), archive.file_len())
    );
    println!(
        "Integrity:       CRC-32 per chunk and header/footer, 128-bit BLAKE3 per file (run `kiln test` to verify)"
    );
    Ok(())
}

fn test(a: TestArgs) -> Result<()> {
    let archive = Archive::open(&a.archive)?;
    let started = Instant::now();
    let stats = kiln::verify_with(&archive, a.threads, a.memory_limit)?;
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
