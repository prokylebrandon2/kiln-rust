//! Kiln for Windows, slice 1: the ZIP engine and a command-line front end to benchmark it.
//!
//! ```text
//! kiln-win a <archive.zip|archive.kiln> <files or folders...> [-l LEVEL] [-t THREADS] [-y]
//! kiln-win x <archive> [names...] [-o DIR] [-t THREADS] [-y]
//! kiln-win l <archive>
//! ```
//!
//! `.kiln` archives go through the Kiln engine (level 0 = store, 1-3 fastest, 4-6 adaptive,
//! 7-9 maximum); everything else is a ZIP.

mod dostime;
mod zip;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use kiln::{Archive, ExtractOptions, PackOptions, Profile};

use zip::{CreateOpts, ExtractOpts, ZipArchive};

const USAGE: &str = "usage:
  kiln-win a <archive.zip|archive.kiln> <files or folders...> [-l LEVEL] [-t THREADS] [-y]
  kiln-win x <archive> [names...] [-o DIR] [-t THREADS] [-y]
  kiln-win l <archive>
options: -l level (0 store, 1 fastest .. 9; zip also 10-12), -t threads (default: all cores),
         -o output folder (default: current), -y overwrite existing files";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("kiln-win: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        return Err(USAGE.to_string());
    };
    match command {
        "a" => cmd_add(&args[1..]),
        "x" => cmd_extract(&args[1..]),
        "l" => cmd_list(&args[1..]),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    }
}

#[derive(Default)]
struct Flags {
    level: Option<u8>,
    threads: usize,
    force: bool,
    out: Option<PathBuf>,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-l" | "--level" => {
                let v = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
                flags.level = Some(v.parse().map_err(|_| format!("bad value for {arg}: {v}"))?);
            }
            "-t" | "--threads" => {
                let v = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
                flags.threads = v.parse().map_err(|_| format!("bad value for {arg}: {v}"))?;
            }
            "-o" | "--out" => {
                let v = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
                flags.out = Some(PathBuf::from(v));
            }
            "-y" | "-f" | "--force" => flags.force = true,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ => flags.positional.push(arg.clone()),
        }
    }
    Ok(flags)
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn printer(label: &'static str) -> impl FnMut(u64, u64) {
    move |done, total| {
        let pct = (done * 100).checked_div(total).unwrap_or(100);
        eprint!("\r{label} {pct:3}%  {}      ", fmt_bytes(done));
    }
}

fn is_kiln_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("kiln"))
}

/// ZIP if the file starts with `PK` (or has the `.zip` extension), otherwise a Kiln archive.
fn is_zip(path: &Path) -> Result<bool, String> {
    let mut magic = [0u8; 2];
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let n = file
        .read(&mut magic)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let zip_ext = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    Ok((n == 2 && &magic == b"PK") || zip_ext)
}

fn cmd_add(args: &[String]) -> Result<(), String> {
    let flags = parse(args)?;
    if flags.positional.len() < 2 {
        return Err(USAGE.to_string());
    }
    let archive = PathBuf::from(&flags.positional[0]);
    let inputs: Vec<PathBuf> = flags.positional[1..].iter().map(PathBuf::from).collect();
    let started = Instant::now();
    if is_kiln_name(&archive) {
        let profile = match flags.level.unwrap_or(5) {
            0 => Profile::Store,
            1..=3 => Profile::Fastest,
            4..=6 => Profile::Adaptive,
            _ => Profile::Maximum,
        };
        let mut options = PackOptions::from_profile(profile);
        options.threads = flags.threads;
        options.overwrite = flags.force;
        let mut progress = printer("packing");
        let stats = kiln::pack_with_progress(&archive, &inputs, &options, &mut progress)
            .map_err(|e| e.to_string())?;
        eprintln!();
        println!(
            "{} files, {} -> {} in {:.2} s",
            stats.files,
            fmt_bytes(stats.raw_bytes),
            fmt_bytes(stats.archive_bytes),
            started.elapsed().as_secs_f64()
        );
        for s in &stats.skipped {
            eprintln!("skipped: {s:?}");
        }
    } else {
        let options = CreateOpts {
            level: flags.level.unwrap_or(6),
            threads: flags.threads,
            overwrite: flags.force,
        };
        let mut progress = printer("packing");
        let stats =
            zip::create(&archive, &inputs, &options, &mut progress).map_err(|e| e.to_string())?;
        eprintln!();
        println!(
            "{} files, {} folders, {} -> {} ({} stored) in {:.2} s",
            stats.files,
            stats.dirs,
            fmt_bytes(stats.raw_bytes),
            fmt_bytes(stats.archive_bytes),
            stats.stored,
            started.elapsed().as_secs_f64()
        );
        for s in &stats.skipped {
            eprintln!("skipped {}: {}", s.path.display(), s.reason);
        }
    }
    Ok(())
}

fn cmd_extract(args: &[String]) -> Result<(), String> {
    let flags = parse(args)?;
    let Some((archive, members)) = flags.positional.split_first() else {
        return Err(USAGE.to_string());
    };
    let archive = PathBuf::from(archive);
    let dest = flags.out.clone().unwrap_or_else(|| PathBuf::from("."));
    let started = Instant::now();
    let mut progress = printer("extracting");
    if is_zip(&archive)? {
        let zip = ZipArchive::open(&archive).map_err(|e| e.to_string())?;
        let options = ExtractOpts {
            dest,
            overwrite: flags.force,
            threads: flags.threads,
            members: members.to_vec(),
        };
        let stats = zip
            .extract(&options, &mut progress)
            .map_err(|e| e.to_string())?;
        eprintln!();
        println!(
            "{} files, {} folders, {} in {:.2} s",
            stats.files,
            stats.dirs,
            fmt_bytes(stats.bytes),
            started.elapsed().as_secs_f64()
        );
        if stats.skipped > 0 {
            eprintln!("skipped {} symbolic links", stats.skipped);
        }
    } else {
        let arc = Archive::open(&archive).map_err(|e| e.to_string())?;
        let mut options = ExtractOptions::new(dest);
        options.overwrite = flags.force;
        options.threads = flags.threads;
        options.members = members.to_vec();
        let stats = kiln::extract_with_progress(&arc, &options, &mut progress)
            .map_err(|e| e.to_string())?;
        eprintln!();
        println!(
            "{} files, {} folders, {} in {:.2} s",
            stats.files,
            stats.dirs,
            fmt_bytes(stats.bytes),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn cmd_list(args: &[String]) -> Result<(), String> {
    let flags = parse(args)?;
    let Some(archive) = flags.positional.first() else {
        return Err(USAGE.to_string());
    };
    let archive = PathBuf::from(archive);
    if is_zip(&archive)? {
        let zip = ZipArchive::open(&archive).map_err(|e| e.to_string())?;
        println!(
            "{:<16} {:>14} {:>14}  {:<8} Name",
            "Modified", "Size", "Packed", "Method"
        );
        for e in zip.entries() {
            let when = e
                .modified
                .map_or_else(|| " ".repeat(16), dostime::format_local);
            let name = if e.is_dir {
                format!("{}/", e.name)
            } else {
                e.name.clone()
            };
            println!(
                "{when:<16} {:>14} {:>14}  {:<8} {name}",
                e.size,
                e.packed,
                e.method_name()
            );
        }
        let files = zip.entries().iter().filter(|e| !e.is_dir).count();
        let raw: u64 = zip.entries().iter().map(|e| e.size).sum();
        println!("{files} files, {}", fmt_bytes(raw));
    } else {
        let arc = Archive::open(&archive).map_err(|e| e.to_string())?;
        println!("{:<16} {:>14}  Name", "Modified", "Size");
        for e in arc.entries() {
            let when = dostime::format_local(e.mtime_secs);
            let name = match e.kind {
                kiln::format::index::EntryKind::Dir => format!("{}/", e.path),
                kiln::format::index::EntryKind::File => e.path.clone(),
            };
            println!("{when:<16} {:>14}  {name}", e.size);
        }
        println!(
            "{} entries, {}",
            arc.entries().len(),
            fmt_bytes(arc.total_raw())
        );
    }
    Ok(())
}
