//! Phase 2 end-to-end tests: the `create / extract / list / test / info` commands, their exit
//! codes, and failure handling (missing input, truncated archives, unwritable output).

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::*;

fn kiln(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("killed by a signal")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn create_list_test_info_extract_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());

    let out = kiln(&["create", "a.kiln", "proj"], dir.path());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains("packed"), "{}", stdout(&out));

    let out = kiln(&["list", "a.kiln"], dir.path());
    assert_eq!(code(&out), 0);
    let listing = stdout(&out);
    assert!(listing.lines().any(|l| l == "proj/ünï cödé/файл.txt"));
    assert!(listing.lines().any(|l| l == "proj/empty.txt"));
    assert!(listing.lines().any(|l| l == "proj/empty_dir/"));

    let out = kiln(&["test", "a.kiln"], dir.path());
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with("OK:"));

    let out = kiln(&["info", "a.kiln"], dir.path());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let info = stdout(&out);
    for needle in [
        "version 2",
        "balanced",
        "Index:",
        "zstd",
        "Files:",
        "Directories:",
        "Chunks:",
        "Ratio:",
    ] {
        assert!(info.contains(needle), "info lacks {needle:?}:\n{info}");
    }
    assert!(
        info.contains("(2 empty)") || info.contains("empty)"),
        "{info}"
    );

    let out = kiln(&["extract", "a.kiln", "--output", "out"], dir.path());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_same_tree(&dir.path().join("proj"), &dir.path().join("out/proj"));
}

#[test]
fn phase1_command_names_still_work_as_aliases() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    assert_eq!(
        code(&kiln(&["pack", "-q", "a.kiln", "proj"], dir.path())),
        0
    );
    assert_eq!(code(&kiln(&["ls", "a.kiln"], dir.path())), 0);
    assert_eq!(
        code(&kiln(&["unpack", "-q", "a.kiln", "-o", "o1"], dir.path())),
        0
    );
    assert_eq!(
        code(&kiln(&["x", "-q", "a.kiln", "-o", "o2"], dir.path())),
        0
    );
    assert_same_tree(&dir.path().join("o1/proj"), &dir.path().join("o2/proj"));
}

#[test]
fn exit_codes_distinguish_failure_kinds() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    assert_eq!(
        code(&kiln(&["create", "-q", "a.kiln", "proj"], dir.path())),
        0
    );

    // 2: usage error (clap).
    assert_eq!(code(&kiln(&["create"], dir.path())), 2);
    assert_eq!(code(&kiln(&["frobnicate"], dir.path())), 2);

    // 3: I/O error (missing archive, missing input).
    let out = kiln(&["list", "missing.kiln"], dir.path());
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(stderr(&out).contains("missing.kiln"));
    assert_eq!(code(&kiln(&["info", "missing.kiln"], dir.path())), 3);
    assert_eq!(code(&kiln(&["create", "b.kiln", "nope"], dir.path())), 3);
    assert!(!dir.path().join("b.kiln").exists(), "no partial archive");

    // 4: not a Kiln archive.
    fs::write(dir.path().join("junk.kiln"), b"definitely not an archive").unwrap();
    for cmd in ["list", "test", "info"] {
        assert_eq!(code(&kiln(&[cmd, "junk.kiln"], dir.path())), 4, "{cmd}");
    }

    // 5: corrupt (bit flip in the middle).
    let mut bytes = fs::read(dir.path().join("a.kiln")).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(dir.path().join("bad.kiln"), bytes).unwrap();
    let out = kiln(&["test", "bad.kiln"], dir.path());
    assert_eq!(code(&out), 5, "{}", stderr(&out));

    // 6: refused (exists, unknown member, bad codec).
    assert_eq!(code(&kiln(&["create", "a.kiln", "proj"], dir.path())), 6);
    assert_eq!(
        code(&kiln(
            &["extract", "a.kiln", "no/such", "-o", "o"],
            dir.path()
        )),
        6
    );
    assert_eq!(
        code(&kiln(
            &["create", "c.kiln", "proj", "-c", "gzip"],
            dir.path()
        )),
        6
    );
}

#[test]
fn truncated_archives_fail_with_corrupt_or_io_codes_and_extract_nothing() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    assert_eq!(
        code(&kiln(&["create", "-q", "a.kiln", "proj"], dir.path())),
        0
    );
    let bytes = fs::read(dir.path().join("a.kiln")).unwrap();
    for keep in [0, 7, 16, bytes.len() / 3, bytes.len() - 1] {
        fs::write(dir.path().join("t.kiln"), &bytes[..keep]).unwrap();
        for cmd in ["test", "list", "info"] {
            let out = kiln(&[cmd, "t.kiln"], dir.path());
            assert!(
                matches!(code(&out), 4 | 5),
                "{cmd} on {keep}-byte truncation gave exit {}: {}",
                code(&out),
                stderr(&out)
            );
        }
        let out = kiln(&["extract", "t.kiln", "-o", "tx"], dir.path());
        assert!(matches!(code(&out), 4 | 5), "{}", stderr(&out));
        assert!(
            !dir.path().join("tx").exists(),
            "nothing extracted for {keep}"
        );
    }
}

#[test]
fn output_errors_are_reported_not_swallowed() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    assert_eq!(
        code(&kiln(&["create", "-q", "a.kiln", "proj"], dir.path())),
        0
    );

    // Destination "directory" is actually a file.
    fs::write(dir.path().join("afile"), b"x").unwrap();
    let out = kiln(&["extract", "a.kiln", "-o", "afile"], dir.path());
    assert_ne!(code(&out), 0);
    assert!(stderr(&out).starts_with("error:"), "{}", stderr(&out));

    // Archive destination inside a directory that does not exist.
    let out = kiln(&["create", "no/such/dir/a.kiln", "proj"], dir.path());
    assert_eq!(code(&out), 3, "{}", stderr(&out));

    // Extracting over existing files is refused without --force.
    assert_eq!(
        code(&kiln(&["extract", "-q", "a.kiln", "-o", "o"], dir.path())),
        0
    );
    assert_eq!(
        code(&kiln(&["extract", "-q", "a.kiln", "-o", "o"], dir.path())),
        6
    );
    assert_eq!(
        code(&kiln(
            &["extract", "-q", "-f", "a.kiln", "-o", "o"],
            dir.path()
        )),
        0
    );
}

#[cfg(unix)]
#[test]
fn skipped_symlinks_give_the_warning_exit_code_but_still_create_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    std::os::unix::fs::symlink("readme.txt", dir.path().join("proj/link")).unwrap();
    let out = kiln(&["create", "--no-links", "a.kiln", "proj"], dir.path());
    assert_eq!(code(&out), 7, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("warning: skipped"),
        "{}",
        stderr(&out)
    );
    assert_eq!(code(&kiln(&["test", "a.kiln"], dir.path())), 0);
}

#[test]
fn large_multi_block_file_round_trips_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big");
    fs::create_dir_all(&src).unwrap();
    // 24 MiB: half text, half noise, so the archive has both compressed and stored blocks.
    let mut data = prose(12 << 20, 11);
    data.extend(noise(12 << 20, 12));
    fs::write(src.join("large.bin"), &data).unwrap();

    let out = kiln(
        &["create", "-b", "1MiB", "-j", "2", "big.kiln", "big"],
        dir.path(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(code(&kiln(&["test", "big.kiln"], dir.path())), 0);
    let out = kiln(&["extract", "big.kiln", "-o", "out"], dir.path());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        fs::read(dir.path().join("out/big/large.bin")).unwrap() == data,
        "large file differs after round trip"
    );
}

// ---- throughput / memory features -------------------------------------------------------

use kiln::codec::CodecSpec;
use kiln::{Archive, ExtractOptions, PackOptions, extract, pack};

fn lib_opts(codec: &str, block_log2: u8, skip: bool) -> PackOptions {
    PackOptions {
        codec: CodecSpec::parse(codec).unwrap(),
        block_log2,
        threads: 2,
        skip_incompressible: skip,
        ..PackOptions::default()
    }
}

#[test]
fn sampler_skips_noise_for_heavy_codecs_and_never_touches_text_or_light_codecs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("noise.bin"), noise(2 << 20, 21)).unwrap();
    fs::write(src.join("text.txt"), prose(2 << 20, 22)).unwrap();
    let inputs = [src.clone()];

    let on = pack(
        &dir.path().join("on.kiln"),
        &inputs,
        &lib_opts("lzma:1", 20, true),
    )
    .unwrap();
    let off = pack(
        &dir.path().join("off.kiln"),
        &inputs,
        &lib_opts("lzma:1", 20, false),
    )
    .unwrap();
    assert_eq!(on.precheck_skipped, 2, "both noise blocks are sampled out");
    assert_eq!(off.precheck_skipped, 0);
    assert_eq!(on.stored_blocks, 2);
    assert_eq!(
        off.stored_blocks, 2,
        "without the sampler they fail to compress and are stored"
    );
    assert_eq!(
        on.archive_bytes, off.archive_bytes,
        "same bytes either way: it only saves time"
    );

    // A light codec never samples.
    let light = pack(
        &dir.path().join("l.kiln"),
        &inputs,
        &lib_opts("zstd:3", 20, true),
    )
    .unwrap();
    assert_eq!(light.precheck_skipped, 0);

    for name in ["on.kiln", "off.kiln"] {
        let archive = Archive::open(&dir.path().join(name)).unwrap();
        let out = dir.path().join(format!("out_{name}"));
        extract(&archive, &ExtractOptions::new(&out)).unwrap();
        assert_same_tree(&src, &out.join("s"));
    }
}

#[test]
fn memory_limit_shapes_the_pipeline_and_rejects_impossible_limits() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());

    let out = kiln(
        &[
            "create",
            "--memory-limit",
            "64MiB",
            "-b",
            "1MiB",
            "-j",
            "4",
            "a.kiln",
            "proj",
        ],
        dir.path(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = kiln(&["test", "--memory-limit", "64MiB", "a.kiln"], dir.path());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = kiln(
        &["extract", "--memory-limit", "64MiB", "a.kiln", "-o", "o"],
        dir.path(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_same_tree(&dir.path().join("proj"), &dir.path().join("o/proj"));

    let out = kiln(
        &["create", "--memory-limit", "1MiB", "b.kiln", "proj"],
        dir.path(),
    );
    assert_eq!(code(&out), 6, "{}", stderr(&out));
    assert!(stderr(&out).contains("memory limit"), "{}", stderr(&out));
    assert!(!dir.path().join("b.kiln").exists());
    let out = kiln(&["test", "--memory-limit", "1MiB", "a.kiln"], dir.path());
    assert_eq!(code(&out), 6, "{}", stderr(&out));
}

#[test]
fn info_reports_how_blocks_are_encoded() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("noise.bin"), noise(256 << 10, 31)).unwrap();
    fs::write(src.join("text.txt"), prose(256 << 10, 32)).unwrap();
    assert_eq!(
        code(&kiln(
            &["create", "-q", "-b", "64KiB", "a.kiln", "s"],
            dir.path()
        )),
        0
    );
    let out = kiln(&["info", "a.kiln"], dir.path());
    let text = stdout(&out);
    let line = text.lines().find(|l| l.starts_with("Chunks:")).unwrap();
    assert!(
        line.contains("8 ") || line.contains("(") && line.contains("zstd"),
        "{line}"
    );
    assert!(line.contains("stored") && line.contains("zstd"), "{line}");
}

#[test]
fn progress_flag_reports_to_stderr_only() {
    let dir = tempfile::tempdir().unwrap();
    make_tree(dir.path());
    let out = kiln(
        &["create", "--progress", "-b", "64KiB", "a.kiln", "proj"],
        dir.path(),
    );
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).contains("100.0%"), "{:?}", stderr(&out));
    assert!(!stdout(&out).contains('%'));
    let out = kiln(&["extract", "--progress", "a.kiln", "-o", "o"], dir.path());
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).contains("100.0%"), "{:?}", stderr(&out));
    // Off by default.
    let out = kiln(&["test", "a.kiln"], dir.path());
    assert!(!stderr(&out).contains('%'));
}

// ---- write failures (file-size rlimit makes write(2) fail with EFBIG) ---------------------

#[cfg(unix)]
fn kiln_with_file_size_limit(args: &[&str], cwd: &Path, kib: u32) -> Output {
    Command::new("bash")
        .arg("-c")
        .arg(format!("trap '' XFSZ; ulimit -f {kib}; exec \"$0\" \"$@\""))
        .arg(env!("CARGO_BIN_EXE_kiln"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

#[cfg(unix)]
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[cfg(unix)]
#[test]
fn create_that_cannot_write_fails_cleanly_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("noise.bin"), noise(1 << 20, 41)).unwrap();
    let before = names(dir.path());

    let out = kiln_with_file_size_limit(&["create", "a.kiln", "s"], dir.path(), 256);
    assert_eq!(code(&out), 3, "stderr: {}", stderr(&out));
    assert!(stderr(&out).starts_with("error:"), "{}", stderr(&out));
    assert_eq!(
        names(dir.path()),
        before,
        "no archive and no temp file may remain"
    );
}

#[cfg(unix)]
#[test]
fn extract_that_cannot_write_fails_without_leaving_truncated_files() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a_small.txt"), prose(10_000, 51)).unwrap();
    fs::write(src.join("noise.bin"), noise(1 << 20, 52)).unwrap();
    assert_eq!(code(&kiln(&["create", "-q", "a.kiln", "s"], dir.path())), 0);

    let out = kiln_with_file_size_limit(&["extract", "a.kiln", "-o", "out"], dir.path(), 256);
    assert_eq!(code(&out), 3, "stderr: {}", stderr(&out));
    assert!(stderr(&out).starts_with("error:"), "{}", stderr(&out));

    // Whatever was written must be complete and correct; nothing half-written, no temp files.
    let mut seen = 0;
    for entry in walk(&dir.path().join("out")) {
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.ends_with(".tmp"), "leftover temp file {name}");
        if entry.is_file() {
            let original = fs::read(src.join(&name)).unwrap();
            assert_eq!(
                fs::read(&entry).unwrap(),
                original,
                "{name} is truncated or wrong"
            );
            seen += 1;
        }
    }
    assert!(seen < 2, "the big file must not have been extracted");
}

#[cfg(unix)]
fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&p) {
            for e in rd {
                let path = e.unwrap().path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn io_errors_are_reported_once_not_twice() {
    let dir = tempfile::tempdir().unwrap();
    let out = kiln(&["list", "missing.kiln"], dir.path());
    assert_eq!(code(&out), 3);
    let msg = stderr(&out);
    assert!(msg.contains("missing.kiln"), "{msg}");
    assert_eq!(
        msg.matches("os error 2").count(),
        1,
        "message repeats itself: {msg}"
    );
}
