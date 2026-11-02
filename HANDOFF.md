# Kiln Rust port: handoff

Goal: a Rust engine (prebuilt .so) for Kiln Android, selectable in the script chooser,
byte-compatible with existing .kln archives. Python engine stays default until Rust passes.
(Note for new sessions: tests/corpus is not shipped in the zips; regenerate it with tools/make_corpus.py.)

## How to resume in a new session
1. Upload: the project zip (kiln-android-*.zip) AND the newest kiln-rust-port-cpN.zip.
2. Say: "Continue the Rust port from HANDOFF.md, next checkpoint is N+1."
3. Rules: no rustc in the sandbox -> every Rust file is verified by a line-for-line Python
   mirror (tests/) compared against the real engine's output. Keep that method.

## Checkpoints (each ends with a zip)
- [x] CP0  Plan + handoff
- [x] CP1  FORMAT.md: exact byte-level spec of header, blocks, table, checkpoints, nonces, index, sets
- [x] CP2  Test corpus (archives made by the real engine + expected output) in tests/corpus/
- [x] CP3  Rust range decoder + block decode (port of kz_rc_dec) + Python mirror, verified on corpus
- [x] CP4  Rust BCJ unfilter + container/table parse + CRC32 + plain list/test/extract, mirror-verified
- [x] CP5  Encryption (scrypt + AES-256-GCM records), mirror-verified
- [x] CP6  C ABI (kiln-rs/include/kiln_rs.h, kiln-rs/src/ffi.rs) + src/app.rs: JSON open/test/extract like kiln_app.py
- [x] CP7  Android: ctypes loader (no JNI needed), Settings -> Rust engine accepts a .so, cargo-ndk CI job
- [x] CP8a WAV unfilters (ids 1-11, 16-62, 64-110) in Rust: src/wav.rs, mirror-verified
- [x] CP8b Sets in Rust: kiln-rs/src/sets.rs written (NOT COMPILED); its Python mirror is verified vs the real kiln_app
- [ ] CP8c MP3 unpack (filter 120, kz_mp3_unpack: ~720 lines of C in kiln_v14.py _MP3_SRC, float model, built with -ffp-contract=off)
- [ ] CP9  (optional) Encoder / pack

## Status log
(update after every checkpoint: what was done, what is verified, what is known broken)
- CP8b (DONE, not compiled): kiln-rs/src/sets.rs is a line-for-line port of the mirror section in app_mirror.py.
  Wiring: lib.rs `pub mod sets;`; app.rs routes directories to sets::set_open / sets::set_run (NO_SETS removed;
  `unsupported`/`fail_json` now pub(crate); FsSink is pub(crate) with `new(...)`, `outer` (set progress mapping),
  pub(crate) abort/rollback); Cargo.toml serde_json has feature "preserve_order" (index key order = entry order).
  All 5 sets parts PASS vs the real kiln_app (sets 27, sets2 10, sets3 40, sets4 19, sets5 incl. MP3-in-set undo);
  regression open 14, test 8, extract 23, unsupported 12, cancel 6, fnmatch 11 all PASS. The old "set folder ->
  unsupported" check was removed from test_app `unsupported`. Rust review by eye only: likely compile nits in sets.rs
  (closure/borrow of `prog`, Option<&Value>::filter). No cargo test file for sets yet (the mirror is the test).
  Known deviations (documented, untested): JSON true as size, ints beyond u64, NaN tokens, lone surrogates in
  index.json; a rename onto an existing directory gives a Rust error text instead of Python's errno text.
  Remaining "unsupported -> Python" cases: archives (or sets with archives) using the MP3 filter 120 -> CP8c.
- CP8b (PARTIAL checkpoint): the previous session's CP8b work was lost (never saved), so it was redone. DONE: sets
  mirror in tests/mirror/app_mirror.py (section "sets (CP8b)": IndexSink, parse_index, read_index_meta, set_open,
  safe_dest, extract_archive, extract_one, set_run; FsSink got `outer` = (finished archives, archives in set)).
  Test part split because scrypt makes it slow; run each separately, set cache in /tmp/kiln_sets_test is reused:
  `python3 tests/mirror/test_app.py <python dir> /tmp/corpus sets|sets2|sets3|sets4|sets5` (corpus: tools/make_corpus.py).
  RESULTS vs the REAL kiln_app: sets 27/27, sets2 (missing/damaged archives) all PASS, sets3 (index variants: v1,
  renamed entry, bad layouts, not json/utf-8, junk/truncated/flipped _index.kln) 40/40, sets5 (cancel, MP3-in-set
  -> unsupported with full undo) PASS after dropping the "cancel after 25" case (test artifact: the real engine emits
  fewer events, so a late stop lets it finish; not a bug). sets4 (odd index paths, duplicate path, other password):
  18 PASS, 0 FAIL, but it hit the time limit before finishing: re-run it with `timeout 290`.
  Behaviour captured (what the Rust must do): open set -> need_password if no pw; test/extract with no pw -> "enter a
  password" (done 0); no _index.kln -> "<dir> is a directory without _index.kln (not a kiln random-name set)";
  items sorted by original path; member filter message "no files in the index match: ..."; safe_dest check
  ("refusing unsafe path: rel"); missing archives are skipped, reported AFTER the success line as
  "N archive(s) listed in the index are missing:" (ok false, done = files + indented lines); clashes message like
  archives; per file: temp dir ".kiln-*" in the target folder, extract the archive there, exactly one entry
  else "unexpected layout in a set archive", rename it to the index name; errors are "<rel> (<archive>): <err>";
  `done` counts printed "  rel" lines plus indented lines inside error text; cancel keeps finished files and
  created folders, removes the temp dir; MP3 filter in an inner archive -> undo this run's new files/folders and
  answer unsupported (avoids doing scrypt twice). Known deviations (documented, untested): JSON true as size, ints
  beyond u64, NaN tokens, lone surrogates in index.json.
  (done, see above) was: write kiln-rs/src/sets.rs as a line-for-line port of that mirror section (add `pub mod sets;` to lib.rs,
  make FsSink/unsupported in app.rs pub(crate) and give FsSink the `outer` field, serde_json feature
  "preserve_order" in Cargo.toml for index key order, route app.rs open/run for directories to sets), then update
  test_engine/app_corpus, rerun open/test/extract/unsupported parts, then CP8c (MP3).
- CP8a: kiln-rs/src/wav.rs (NEW): wav_spec, wav_layout (RIFF header walk), kz_wav16_dec / kz_shuf_dec / kz_int_dec /
  kz_i2f ports, and the streaming `Wav` decoder (same state machine as python `Wav`, decode direction). container.rs:
  `Unfilter` now has a Wav variant and feed/finish return Result (WAV can raise "corrupt data: bad WAV header for the
  filter"); make_unfilter: MP3 (120) -> error (app then answers "unsupported" -> Python), BCJ -> Bcj, else Wav::new.
  wav_spec_valid now calls wav::wav_spec (single source). app.rs: only MP3 archives (and sets) are "unsupported" now.
  Mirror: tests/mirror/wav_mirror.py; test: `python3 tests/mirror/test_wav.py <kiln_v14.py> layout|kernels|stream`
  (each < 90 s). ALL PASS vs the REAL engine: wav_layout on 10692 header cases (truncations, corruptions, LIST chunk,
  extensible, 0xFFFFFFFF data length, trailing bytes); wav_spec for ids 0..129; decode kernels for every valid filter id x
  1/2/3 channels x lengths 0,1,2,331 (== real C == original), and on ARBITRARY bytes (NaN/inf floats included);
  streaming decode of 18 WAV variants through 404 filter/piece-size runs (58 distinct filters, pieces of 1, 997, 65537,
  whole) at WAV_FRAMES = 2^18, 700 and 64 (chunk boundaries; the Rust constant is 2^18); error paths and unknown ids give
  the same messages. test_app.py part `unsupported` now checks that a real WAV archive open/test/extract (all, single
  member, other member) gives the same JSON and the same extracted tree as the real kiln_app and equals the source WAV;
  open (14), test (8), extract (23) re-run: PASS. NOT COMPILED. Known: the Rust test for wav is only the Python mirror
  (no cargo test file yet).
- CP7: the app side. The Rust crate now lives in the Android project under `rust/` (so one project zip is enough
  to resume). Changed Android files (also copied in the cp7 zip under android-changes/):
  app/src/main/python/kiln_rs_engine.py (NEW: ctypes loader, probe(), run() -> dict or None = "use Python"),
  kiln_scripts.py (install role "engine" -> install_engine: ELF check, unique candidate name, load + `version`
  probe must answer {"kiln":"rs-..."}, stored read-only as scripts/engine.so, no restart; remove("engine");
  status has engine/engine_version/engine_active; boot configures it), kiln_app.py (open_archive/extract/test try
  the engine first, only when the main script is the built-in KLNA one; None -> old Python path; version() reports
  engine), SettingsActivity.java (row "Rust engine" + engineSheet; NOT COMPILED), .github/workflows/build.yml (new
  job `rust-engine`: make corpus, cargo test, cargo-ndk build for arm64-v8a + x86_64, artifact `kiln-rust-engine`;
  continue-on-error so the APK never depends on it). Rust: app.rs returns {"unsupported":true} for sets and for
  archives with WAV/MP3 filters BEFORE touching anything, and the app then falls back to Python.
  Design note: no JNI. kiln_app is Python (Chaquopy) and already loads the C helper with ctypes, so the Rust .so
  is a plain C-ABI library called with ctypes.
  Mirror/tests: tests/mirror/test_engine.py <python dir> <rust dir> (uses tools/stub_kiln_rs.c, built with gcc, as a
  stand-in for libkiln_rs.so): 27 checks PASS (non-ELF / no symbol / wrong protocol refused, nothing left behind,
  read-only file, status, dispatch of open/test/extract incl. utf-8 and progress events, cancel reaches the engine,
  progress=None, custom main script never uses Rust, "unsupported" falls back, a NULL result switches the engine
  off with a note, boot re-enables it / notes a damaged one, remove). test_app.py gained part `unsupported` (a real
  WAV archive + a set folder): PASS. Bug found by the test and fixed: dlopen caches by path, so every candidate
  gets a unique file name. Re-run after the changes: test_app open/fnmatch PASS.
  NOT YET DONE / known: preview/cached_extract still go through Python (they use unpack via the CLI);
  nothing has run on a phone; Java and Rust NOT COMPILED. If the CI job fails, paste its log into a new session:
  fixing compile errors is the first job of CP8.
- CP6: kiln-rs/src/app.rs (pure Rust JSON layer: open_archive / test / extract, same JSON objects and
  message texts as kiln_app.py; fnmatch + glob.escape member patterns, safe-path/clash checks, FsSink with
  mtime/mode, abort on error, rollback on cancel, progress events {frac,done,total,file}) and kiln-rs/src/ffi.rs
  (C ABI: `char *kiln_rs_call(op, args_json, cb, user)` + `kiln_rs_free`; ops version|open|test|extract; cb gets a
  progress JSON or NULL = "cancelled?", non-zero = cancel; panics are caught; Cargo panic = "unwind" now).
  kiln-rs/include/kiln_rs.h documents it. serde_json is now a normal dependency. Sets (folders) -> error "sets are not
  supported by the Rust engine yet" until CP8.
  Mirror: tests/mirror/app_mirror.py; test: `python3 tests/mirror/test_app.py <dir with kiln_app.py+kiln_v14.py>
  tests/corpus open|test|extract|cancel|fnmatch` (run parts separately, each < 90 s). The corpus is NOT in the zip:
  `python3 tools/make_corpus.py <kiln_v14.py> tests/corpus` (about 1 min) first.
  ALL PASS vs the REAL kiln_app: open (14 cases: plain, ckpt, enc ok / no password / wrong password, ckpt_enc, bad_*,
  not-an-archive, tiny, old KLN9, unknown flag, empty), test (8), extract (23: all, one file, dir, 2 members, empty
  file, no match, glob chars, unsafe-looking member, clash with/without overwrite incl. kiln_app's quirk that
  indented lines count as `done`, ckpt partial, bad_*, encrypted incl. wrong/no password; extracted trees compared
  byte-for-byte + mtime + mode), cancel (3: result and nothing left behind), fnmatch == Python fnmatch on 40000
  random pairs. kiln-rs/tests/app_corpus.rs is the `cargo test` version. NOT COMPILED.
- CP5: kiln-rs/src/crypto.rs (derive_key = scrypt of NFKC password, AES-256-GCM CryptSource with the
  AAD-0/AAD-1 end-marker logic) + container.rs now opens encrypted archives (header with KDF params, table stream 1,
  data stream 0 starting at the checkpoint's record counter). Cargo deps added: aes-gcm 0.10, scrypt 0.11
  (Params::new has a `len` arg in 0.11: adjust if cargo picks another API), unicode-normalization.
  Mirror: tests/mirror/container_mirror.py (uses python `cryptography` + hashlib.scrypt like the real engine);
  tests: `timeout 280 python3 tests/mirror/test_crypto.py <kiln_v14.py> tests/corpus <part>` with parts
  store | ckpt | bad | full. ALL PASS: enc_store and enc (table + extracted bytes == real engine), ckpt_enc
  (table, 4 checkpoints, partial extract from a checkpoint with its record counter), error texts == real engine for
  bad_enc_flip, wrong password, bad KDF settings, short header, truncated, damaged footer. NOT COMPILED.
- CP4: kiln-rs/src/bcj.rs (kz_bcj decode, x86+AArch64, 1 MiB block streaming) and kiln-rs/src/container.rs
  (header/footer, records, StreamReader, parse_table + CKPT, Extractor, Archive::open/test/extract; plain archives
  only; encrypted -> error until CP5; WAV/MP3 filters -> error until CP8). Mirrors: tests/mirror/container_mirror.py,
  test with `python3 tests/mirror/test_container.py <kiln_v14.py> tests/corpus` (SLOW: ~10+ min on 1 core, run it
  in the background: `nohup ... > log &`).
  VERIFIED (passed): BCJ mirror == real kz_bcj for both filters (elf, random, tiny, empty, 3 pos0 values) and
  streaming across 1 MiB blocks; plain.kln and ckpt.kln: table == expected.json (incl. 4 checkpoints), test() CRCs OK,
  extracted bytes == real engine, partial extracts on plain.kln.
  FIXED after a failing test: the sink got indexes relative to the checkpoint slice; Extractor now takes `base`.
  RE-RUN after the fix (tests/mirror/test_container_part2.py, parts `ckpt` and `bad`): partial extract from a
  checkpoint (ckpt.kln: file 5 / file 3, correct indexes + CRCs) PASS; bad_truncated and bad_flip_data error texts ==
  real engine PASS. bad_enc_flip is checked in CP5. (Run long tests in the FOREGROUND with `timeout 280`: background
  jobs are killed between commands.)
  NOT COMPILED.
- CP3: kiln-rs/src/rc.rs (port of kz_rc_dec: models, mixer, tables). tests/mirror/rc_mirror.py is its
  line-for-line Python mirror: `python3 tests/mirror/test_rc.py tests/corpus` -> 72/72 vectors match the real
  engine (all 6 flag modes, with history); `python3 tests/mirror/fuzz_rc.py <kiln_v14.py> tests/corpus 250`
  -> 250/250 identical results AND identical error codes on corrupted payloads. kiln-rs/tests/rc_corpus.rs
  is the `cargo test` version. NOT COMPILED (no rustc in sandbox): first thing to do when a toolchain exists:
  `cd kiln-rs && cargo test` and fix any compile errors.
- CP2: tools/make_corpus.py <kiln_v14.py> <outdir> builds tests/corpus/: plain, enc, enc_store, ckpt, ckpt_enc,
  3 bad_* archives, expected.json (tables, crcs, ckpts) and 72 block vectors (blocks/NAME.payload/.hist,
  all 6 flag modes, a=no history, b=with history). Password: correct-horse-battery. Generated by the REAL engine
  and engine roundtrip-checked. Re-run it if the corpus is missing.
- CP1: FORMAT.md + tools/accel_reference.c written from kiln_v14.py (decoder-side only).
- CP0: plan written. Nothing compiled. No Rust toolchain in sandbox.
