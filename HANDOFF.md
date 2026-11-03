# Kiln Rust port: handoff

Goal: a Rust engine (prebuilt .so) for Kiln Android, selectable in the script chooser,
byte-compatible with existing .kln archives. Python engine stays default until Rust passes.
(Note for new sessions: tests/corpus is not shipped in the zips; regenerate it with tools/make_corpus.py.)

## How to resume in a new session
1. Upload: the project zip (kiln-android-*.zip) AND the newest kiln-rust-port-cpN.zip.
2. Say: "Continue the Rust port from HANDOFF.md, next checkpoint is N+1."
3. Rules: check `rustc --version` first. Sessions up to CP8b had NO rustc and used line-for-line Python mirrors
   (tests/mirror/). Since CP8c a toolchain existed (rustc 1.97 in the sandbox) and ALL Rust compiles and is tested
   directly, so prefer real tests: `cargo test --release` (needs the corpora below) and `tests/rs_so/` (drives the compiled
   libkiln_rs.so through ctypes against the REAL kiln_app). If there is no rustc again, fall back to the mirrors.
4. Corpora are not in the zips. Regenerate (needs gcc; the MP3 one also ffmpeg with libmp3lame/libshine):
   `python3 tools/make_corpus.py ../app/src/main/python/kiln_v14.py tests/corpus` (secs)
   `python3 tools/make_mp3_corpus.py ../app/src/main/python/kiln_v14.py tests/mp3corpus` (about 1 min)
   `python3 tools/make_mp3_fuzz.py ../app/src/main/python/kiln_v14.py tests/mp3corpus 90`
   (run all python tools with `python3 -I`; sandbox cargo may need old dependency pins only if rustc < 1.80)

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
- [x] CP8c MP3 unpack (filter 120): kiln-rs/src/mp3.rs + mp3_tables.rs; byte-exact vs the real C on 33 MP3 files, 900 corrupted blobs agree
- [x] CP9a Range ENCODER in Rust: rc.rs `encode_block` (port of kz_rc_enc/enc_tok), byte-identical to the real C on 306 vectors
- [x] CP9b Match finder + optimal parser (lz.rs), chunk classifier (rnd.rs), build_blocks glue (pack.rs): identical to C/Python
- [x] CP10a Custom main script + injectable engine REMOVED (Python, Java, CI, README). libkiln_rs.so is bundled
  (CI: cargo-ndk -> app/src/main/jniLibs/<abi>/). KLN9/KLN2 references removed. Only KLNA is recognised.
- [x] CP10b CP9c step 1: forward BCJ in Rust (bcj.rs: bcj_encode, BcjEnc, round-trip unit test). NOT COMPILED.
- [x] CP10c CP9c step 2: container writer kiln-rs/src/packer.rs (serial; plain/encrypted/store, checkpoints, BCJ 121/122). NOT COMPILED.
- [x] CP10d CP9c step 3: walk.rs (collect, bcj_kind, pick_bcj) + packapp.rs and the FFI `pack` op. NOT COMPILED.
- [x] CP10e CP9c steps 4+5: kiln_app.pack calls Rust `pack` first (Python only when Rust says unsupported / is missing); packer threads + fit_segment + auto_jobs. NOT COMPILED.
- [x] CP11a WAV encode kernels (wav16_enc, shuf_enc, int_enc, f2i) + `Wav::new_enc`/`enc_chunk` in wav.rs, with a cargo round-trip test. NOT COMPILED.
- [x] CP11b WAV filter picker (`wav::pick_wav_filter`, port of pick_wav_filter + _float_scale/_float_verified/_entropy_bits) and packer.rs `Fwd` enum (BCJ or WAV) so the Rust packer packs WAV files; packapp no longer answers "unsupported" for WAV. NOT COMPILED.
- [x] CP11c MP3 forward filter: kiln-rs/src/mp3parse.rs (port of parse_file/dec_gran/parse_side/tries), mp3enc.rs (range ENCODER + model, port of code_parsed with dec=0, mp3c_compress, `mp3_pack` with the python 0.97 + decode-verify checks), packer.rs `pack_file` handles filt 120 (falls back to BCJ/none), packapp sets 120 for .mp3 16 KB..64 MB (not store; before the `pre` rule). Only `each` mode is still "unsupported" -> Python. NOT COMPILED.
- [x] CP12a "one archive per file" (-i / `each`) in Rust: kiln-rs/src/packeach.rs (cmd_pack_each + write_index + check_set_target), packapp.rs dispatches `each` and shares `filter_for`/`plan`. NOT COMPILED.
- [x] CP12b tests/rs_so/test_pack_each_so.py (Rust `pack` each=true plain + encrypted set vs the real engine/kiln_app; wrong password, rerun, -f) added to the CI rust-engine job. NOT RUN (no rustc).
- [x] CP12c README note 6 (cache in Settings): SettingsActivity row is now "Cache" and sums/clears getCacheDir() (preview, incoming, edit, pick, ...) plus getExternalCacheDir() (priv staging) through the existing Python cache_size/clear_cache (called once per folder). JAVA NOT COMPILED. Caveat: Clear also deletes scratch/edit copies, so do not clear while a file is open for editing. Other notes (12, 13, 7-11, 2, 3) untouched.
- [x] CP12e README note 12 (generic, replaces the icon-only CP12d change): Sheet.java has `static Runnable nextBack`; a Sheet takes it in its constructor and runs it after dismissal if the person backed out (Back, tap outside, or a button with no action = Cancel/OK). MainActivity.menuSheet sets nextBack just before onMenu(id) (clears it afterwards) with a closure that restores the selection snapshot, redraws and calls showMenu(). So every sheet a menu item opens (Change icon, Open options, Properties, Rename, Archive info, Extract, ...) returns to the menu on Back/Cancel; choosing an action closes as before. Risks to check on a device: a sheet that is created later/async never gets it (fine); sheets whose OK button has a null action now also return to the menu (Properties/Info OK) - intended, change if you'd rather they close. JAVA NOT COMPILED.
- [x] CP12f README note 13 (partly): MainActivity.ProgressUi no longer shows its sheet at once; it is shown 250 ms after the job starts (`showLater`, SHOW_AFTER_MS) and `close()` cancels that and skips the dismiss animation if it never appeared. So "Preparing...", "Opening..." etc. no longer flash for jobs that finish quickly (small .kln archives, private-folder staging). Side effect: for the first 250 ms the screen is not blocked. NOT done: the flash seen between holding a folder -> three dots -> "Change icon..." (that is menu sheet fade-out + picker fade-in, not a loading sheet; needs a device to look at). JAVA NOT COMPILED (no Android SDK, no rustc in this sandbox either).
- [ ] CP9c (NEXT: compile + run `cargo test --release` as soon as a toolchain exists and fix nits; then drop Python pack/engine (kiln_app.pack fallback, kiln.collect duplicate); then README notes 12, 13, 6, 7, 8, 9, 11, 10, 3, 2) rest of the packer: forward filters (BCJ/WAV/MP3 pack), container writer (header, table, checkpoints, encryption), FFI pack op, multithreading

## README.txt notes (user's to-do list, kept next to the checkpoints in Checkpoint/README.txt)
Status after this session (Java cannot be compiled here: no Android SDK; Python/Rust parts are tested):
- 4 (parallelism): DONE in kiln_v14.py: if worker processes cannot be created, submit fails, or a worker dies, the Encoder
  switches to a THREAD pool (make_thread_pool; the C helper releases the GIL; tables pre-warmed). Tested on 2 cores: 14 MB
  text+samples: serial 10.2 s, processes 5.4 s, forced-process-failure threads 5.4 s, archives byte-identical to processes.
  Note format: " [2 threads - worker processes failed: <why>]". MainActivity.packResult shows a "Parallel" row; Settings text updated.
  NOT yet known: WHY the app's processes fail (needs a phone run: the Parallel row now tells). The "memory leak / killed" part
  (OOM) is not investigated: check per-worker memory = job buffers (SEGMENT) x jobs.
- 1 (faster pack/unpack/check): unpack/test/list are in Rust (CP5-CP8c). Pack = CP9 (not started).
- 2, 3, 5-13: not started (Android UI/Java work, SAF Termux home, archive viewing, cache listing, benchmark limits, cancel
  speed, search/regex, coloured folders, recycle bin, modal back-stack, stutter). Do them in a session with an Android build.
  Suggested order: 12 (modal back), 13 (stutter), 6, 7, 8, 9, 11, 10, 3, 2.
## Status log
- CP12f (NOT COMPILED): see checklist. Only ProgressUi in MainActivity.java changed. Other loading sheets (none found by grep for "Loading"/"Please wait") do not exist; the only "Loading..." is the empty-folder text, already delayed.
- CP12a (NOT COMPILED, no rustc/network): packeach.rs = port of cmd_pack_each. Plain (no -e): mirrors the layout, OUT/<rel>.kln, entry stored under the file's basename (python packs [full path]). Encrypted (-e): random names (token_hex(8)+".kln"), an existing _index.kln is read first (read_index_meta, wrong password fails before any file work) and reused so reruns resume; todo is shuffled with /dev/urandom; the index (`index_json` = json.dumps(sort_keys=True), ensure_ascii, default separators; unit test uses a string made by the real python) is written BEFORE the archives through a temp file OUT/.kiln-index-<hex>.tmp, entry rel \"index.json\", mode 0600, `store` carries over like python. Messages equal python's: \"nothing to do: all N output(s) already exist (use -f to overwrite)\", \"index updated: N entr(ies) in OUT/_index.kln (sizes and dates added); M archive(s) already exist\", \"N archive(s) in OUT: A -> B (x% of original) in Ts [encrypted: AES-256-GCM] [no compression] [random names, index: _index.kln], K skipped (already exist)\". Errors: \"<rel>: <err>\". Password rule: min length only when the set has no index yet (python get_password confirm). Progress events: frac by bytes of the archives still to build, done = finished archives, total = all collected files, file = current rel. Result has `packed` (only the files that went into an archive this run) for delete-after-pack. kiln_rs.h now documents the `pack` op. packapp.rs: `filter_for(item, store)` and `plan(total, store, jobs_arg)` extracted; `each` is dispatched BEFORE the dest-exists and password checks. Known differences: scrypt runs once per archive (like python), archives are built one after another (no parallelism across files), `kiln.collect` in kiln_app.pack still runs once first. Tests to run first when a compiler exists: `cargo test --release packeach` (json_matches_python, plain_mirrors_layout_and_skips_existing, encrypted_set_has_index_and_resumes, set_target_rules), then compare with python: pack the same folder with `kiln_v14.py pack -i` and the Rust `pack` (each=true) and check that listing/extraction of the Rust output with the real kiln_app gives the same tree (names are random with -e: compare the decrypted index paths). tests/rs_so should get a pack/each case.
- CP11c (NOT COMPILED, no rustc/network): mp3.rs internals made pub(crate) (Gran/Frame/Model/Mixer/consts/helpers; Gran got `stuff_off`, Frame `gap_n`/`gap_off`, unused by the decoder).
  mp3parse.rs keeps one ~10 KB Frame per parsed frame (like C; a 64 MB MP3 can need > 1 GB: consider a streaming/two-pass parse or a lower MP3_MAX
  on phones; the Python side's `start_mp3_ahead` memory check is NOT ported). MP3 packing is serial and cannot be cancelled mid-file.
  The encoder mirrors C exactly, including that code_coefs leaves magnitudes in g.xr (only abs() is read later). Test to run first:
  `KILN_MP3_CORPUS=tests/mp3corpus cargo test --release --test mp3_corpus pack_matches_real_engine` (compares with the NAME.blob files made by the real
  kz_mp3_pack); then archive-level compare vs python for a folder with MP3s. Parser edge cases to fuzz against C: tools/make_mp3_fuzz.py corpora.
- CP11a+b (NOT COMPILED, no rustc and no network in that sandbox): see the two checklist lines above. Details: wav.rs encode kernels are
  line-for-line ports of kz_wav16_enc / kz_shuf_enc / kz_int_enc / kz_f2i (the C source is inside kiln_v14.py near line 6060, not in
  tools/). `Wav::new_enc(fid,size)` streams like the decoder (same header walk, WAV_FRAMES chunks); `enc_chunk` = python `_enc`
  (fint pads with zeros; f2i failure -> "float samples are not whole-number audio (file changed?)"). pick_wav_filter mirrors the
  python loop and tie order (strict `<`, candidate order kept, entropy summed in the same order); an f64 log2 difference vs
  Python's math.log2 could in theory flip a near tie. packer.rs: `Fwd` enum; the pre-check in pack_to accepts any id wav_spec knows.
  TO CHECK FIRST when a compiler exists: `cargo test --release` (new tests `enc_tests::kernels_round_trip`, `float_round_trip`),
  then compare Rust-packed WAV archives byte for byte with the real kiln_v14 pack (16/24/32-bit, float32-as-int, mono/stereo),
  and compare `pick_wav_filter` ids against python on the same files. The "note: WAV filter not used" stderr message is not ported.
  Still "unsupported -> Python": MP3 files (16 KB..64 MB, not store) and `each` mode.
- CP10e (NOT COMPILED, no rustc): packer.rs Encoder now runs segments on std threads (at most `jobs` at a time, oldest
  written first, so the archive is identical to the serial one: unit test `threaded_equals_serial` compares 1 vs 3 jobs).
  `fit_segment` (4..16 MiB) and `auto_jobs` (cores <= 8, and 60% of MemAvailable / worker_bytes; worker_bytes uses 250 MB tables
  + 2*(hist+segment) + 8*segment, an UNMEASURED estimate; this is the README note 4 OOM suspect) are ported. packapp reads
  optional arg `jobs` (0 = auto); the message gets " [N threads]" when N > 1. Cancel is only polled when a segment finishes
  (README note 8 still open: poll inside build_blocks / lz). A cancelled pack leaves running threads to finish on their own.
  Python: kiln_app.pack() first calls `_rust("pack", ...)` with jobs = Settings value if > 1 else 0 (auto); on {"unsupported"} or
  no engine it runs the old Python path unchanged. kiln.collect still runs once in Python first (a cheap duplicate; remove it
  when the Python path goes). Delete-after-pack uses the `packed` list from Rust.
- CP10d (NOT COMPILED, no rustc): walk.rs = collect (same order as Python: verified by running the real kiln_v14.collect on
  the same layout; symlinked folders skipped), ext rules, bcj_kind, pick_bcj (uses miniz_oxide 0.8 zlib level 6 instead of
  zlib: the choice can differ from Python only for files right at the 0.995 threshold; NEW Cargo dependency, Cargo.lock
  not updated: run cargo without --locked once). packapp.rs = `pack` op: args {items,dest,encrypt,store,overwrite,password,each};
  returns {ok,message,done,packed,warnings} (message text like Python's summary line, without the core note) or
  {"unsupported":true} for `each`, MP3 (16 KB..64 MB, not store) and WAV (>=128 KB, not pre, not store). Progress events carry
  frac (bytes) + total files but done=0 and file="" for now (the encoder does not know file boundaries yet: TODO).
  Cancel returns {ok:false,cancelled:true}. NOT done: kiln_rs.h docs for `pack`; Python side calls nothing yet.
- CP10c (NOT COMPILED, no rustc): packer.rs = port of Encoder (segments, history WINDOW, checkpoints every CKPT_SPAN),
  PlainSink/CryptSink (nonce stream/counter, AAD header+final), build_table_bytes (hex checked against the real Python,
  see unit test `table_bytes_equal_python`), cmd_pack single-archive flow (header, data stream, table stream through
  its own Encoder, footer; writes dest.tmp then renames; salt from /dev/urandom). `pack_archive(dest, &[PackFile], &PackOpts, progress)`.
  Progress callback returns true to cancel (Err "cancelled", tmp removed). Unit test `archives_read_back` packs 300 KB
  (plain, BCJ x86/arm64, encrypted+store; 100 KB segments) and reads back with Archive::open + test().
  Differences from Python still open: no threads, no `fit_segment`, WAV/MP3 filters refused ("a filter the Rust packer does not
  support"), no pick_bcj (caller passes the filter id), no walk/collect, no FFI op. Compare bytes vs real Python in CI/next session.
- CP10b (NOT COMPILED, no rustc): bcj.rs gained `bcj_encode` (port of kz_bcj dec=0: x86 right to left, AArch64 forward),
  `BcjEnc` (1 MiB independent blocks, mirror of `Bcj`) and a `cargo test` round trip (both filters, sizes 0..1 MiB+777).
  The x86 logic was also checked as a Python mirror round trip. Still to compare against the real C encoder in CI/next session.
  NEXT (step 2): packer.rs container writer: header, per-file records, table, checkpoints, encryption (see FORMAT.md and
  kiln_v14.py Encoder), using pack.rs build_blocks + BcjEnc. Then FFI `pack` op, then wire kiln_app.pack.
- CP10a (DONE, NOT COMPILED: no rustc, no network in that sandbox; Java not compiled either): kiln_scripts.py is now
  just `clean_old(files_dir)` (deletes scripts/); kiln_boot.init(helper, files_dir, rs_path) configures the bundled
  Rust lib; kiln_app: no signature settings, no "old" kind, `_rust` no longer checks for a custom script;
  SettingsActivity: SCRIPTS section, scriptSheet/engineSheet/installScript/askMagic/changeMagic/restartNote removed;
  MainActivity: "old" kind removed, About has no "Main script" row, passes nativeLibraryDir/libkiln_rs.so to init;
  build.yml: the build job now builds libkiln_rs.so before the APK (the rust-engine job only tests).
  Transcript 8/9 work (Rust packer.rs, BCJ forward filter, Python encoder-order fix in kiln_v14) was NEVER SAVED and is
  not in this zip. Redo it: container writer + forward BCJ + FFI `pack` op, wire kiln_app.pack to it, then drop the
  Python fallbacks. NOT done: README notes 1-13 except the engine part of 1.
  Still check: tests/mirror/test_engine.py and tests/rs_so still assume an installable engine / custom scripts.
(update after every checkpoint: what was done, what is verified, what is known broken)
- CP9b (DONE, COMPILED AND TESTED): src/lz.rs = port of kz_lz (hash chains, binary-tree finder bt_run, long-match table,
  optimal parser opt_window with live price learning, raw/literal-only/sparse chunk handling, lazy/greedy levels),
  src/rnd.rs = port of kz_rnd (same hand-made log2 and float operation order), src/pack.rs = port of build_blocks/_code_parts/
  _code_block (mode trial, TRIAL_TOKENS cut, MODE_TIE, LP re-parse). rc.rs got `enc_tok` (one token, shared by the encoder and
  the parser's model learning) and its model types are pub(crate). Notes: use6 is always false in the C build (BTMODE 2), so the
  6-byte chain is not ported; the buffer passed to lz77 must be padded with 16 zero bytes (C reads the NUL after a bytes object);
  `son` is initialised to -1 (C leaves it uninitialised but never reads unwritten nodes). Hot byte/table reads use unchecked access
  (see SAFETY comment in lz.rs) + prefetch hints (x86_64 _mm_prefetch, aarch64 `prfm`; both only affect speed).
  Tests (all PASS, vectors are made by the REAL engine and are NOT shipped; CI makes them):
    tools/make_lz_vectors.py <kiln_v14.py> tests/lzvec  (~60 MB, 360 cases: 11 data kinds x starts x 5 parameter sets x lp 0/1/2,
      all 4 chunk classes, raw runs, + 150 fuzz cases of tiny/odd inputs) -> `cargo test --release --test lz_vectors`:
      token stream AND raw-run list identical to C kz_lz in all 360; chunk classes identical to C kz_rnd on 166 buffers.
      Also run in a debug build (overflow checks + bounds debug_asserts) for the 150 fuzz cases and 7 more subsets: no panic.
    tools/make_blk_vectors.py <kiln_v14.py> tests/blkvec  (needs lzvec first) -> `cargo test --release --test blk_vectors`:
      20 cases (incl. history offsets, stored runs, multi-block, the LP re-parse path) identical to Python build_blocks.
    enc_vectors (CP9a) still 306/306.
  Speed (1.16 MB mixed file, 1 core, same machine): lz77 Rust 0.52 s vs C 0.49 s; rc encode of all 6 modes 0.245 s both;
  rnd 0.006 s both; build_blocks end to end Rust 0.62 s vs Python-calling-C 0.61 s (parity).
  So single-thread speed is about C parity; the Rust packer's gain has to come from threads and from no Python in the loop
  (CP9c): e.g. the 6 mode trials of a block are independent and can run on 6 threads; segments can run in parallel.
  aarch64-linux-android: `cargo check` passes with the new code (asm prefetch included); NOT run on a phone.
  NEXT: CP9c = BCJ/WAV/MP3 forward filters, container writer, FFI `pack` op, threaded mode trials; compare archives byte for byte
  with the real kiln_v14 `pack` on the corpus inputs.
- CP9a (DONE, COMPILED AND TESTED): toolchain note: this sandbox had NO rustc this time; got rustc 1.99.1 + rust-std (x86_64 and
  aarch64-linux-android) from conda-forge packages (conda.anaconda.org is reachable; static.rust-lang.org is not): download the .conda
  files (zip containing pkg-*.tar.zst), extract with python zstandard, put bin/ on PATH and lib/ on LD_LIBRARY_PATH.
  Verified first: whole crate builds, `cargo test --release` passes, and `cargo check --release --target aarch64-linux-android`
  PASSES (the Android type-check that earlier sessions could not do; linking still needs the NDK, CI does that).
  New: rc.rs `pub fn encode_block(tok:&[u64], buf, pos0, lp, cap) -> Option<(raw_len, payload)>` = exact port of kz_rc_enc
  (range encoder with carry/cache, literal/mixer-literal/match/rep/dist coding, same token rules; None = "store it instead",
  also for a distance slot > 26 where C would read out of bounds). Test: tools/make_enc_vectors.py <kiln_v14.py> tests/encvec makes
  306 vectors with the REAL engine (9 data kinds, 2 start offsets, literal-position 0/1 parses, all 6 modes 0,1,2,4,5,6);
  `cargo test --release --test enc_vectors`: payload bytes identical to C for all 306. (tests/encvec is ~60 MB, not shipped.)
  NOT done: kz_lz (match finder, C lines 1237-1415 of tools/accel_reference.c), kz_rnd, build_blocks/_code_block mode choice,
  forward filters (BCJ/WAV/MP3 pack), container writer, FFI ops for pack. No speed benchmark of the encoder was taken.
  NEXT: CP9b = port kz_lz + kz_rnd and compare token streams with the real lz77() on the same vectors, then the mode-choice glue.
- CP8c (DONE, COMPILED AND TESTED): the whole Rust crate compiled for the first time (rustc 1.97, no source changes were
  needed for any earlier checkpoint) and everything passes.
  New: kiln-rs/src/mp3.rs = decode side of kz_mp3_unpack (the model is integer-only, not float: range decoder, state maps,
  mixer, count1/big-value coefficient model, side info, bit-reservoir rebuild). STREAMING port: frames are written into the
  output as they are decoded and only the run's bit stream is kept (peak RSS 29 MB for a 7.2 MB MP3; the C code needs ~10x
  the file). Speed equals the C code (0.6 s for 7.2 MB). Tables: mp3_tables.rs is GENERATED from kiln_v14.py by
  tools/gen_mp3_tables.py <kiln_v14.py> <out.rs> (regenerates byte-identical). QMT2 in mp3.rs is a literal copy of the C table.
  container.rs: `Unfilter::Mp3(Mp3Un)` (collects the packed bytes, decodes in finish(); error text
  "corrupt data: bad packed MP3" like python); make_unfilter(120) now succeeds. app.rs / sets.rs keep the "unsupported" path
  but only for a filter the engine cannot build (none known); message texts changed ("a filter the Rust engine does not support").
  Tests (all PASS): `cargo test --release` = rc_corpus 2, app_corpus 7, mp3_corpus 3 (33 real MP3 files made by ffmpeg lame/shine:
  CBR/VBR/ABR, mono/stereo/joint, 32/44.1/48 kHz, ID3 tags, junk before/after, truncated, concatenated, bit-flipped, CRC-flag;
  packed by the REAL kz_mp3_pack; Rust output == original; plus 900 corrupted blobs: accept/refuse AND bytes equal to the real
  kz_mp3_unpack). Also run in a debug build (overflow checks on): no panic. tests/rs_so/run.py runs the old test_app.py parts
  against the compiled .so: open 14, test 8, extract 23, cancel 6, fnmatch 11, unsupported(WAV) 11, sets 27, sets2 11, sets3 40,
  sets4 19, sets5 10: ALL PASS vs the real kiln_app (this is the first time sets.rs/wav.rs/crypto.rs/container.rs ran as Rust).
  tests/rs_so/test_mp3_so.py: real plain + encrypted archives and a real encrypted set holding 7 MP3s: open/test/extract all/
  one/two members == real kiln_app (JSON and trees) and == the source bytes; 24 damaged archives give the same verdict+message.
  CI (build.yml job rust-engine) now also installs ffmpeg, makes the MP3 corpus and runs tests/rs_so against the built .so.
  NOT verified: cross-compile for Android (the sandbox cannot add the aarch64 target; CI does it), anything on a phone.
  Known limits: the MP3 frame fit check happens per frame (the C code checks once per run) - same accept/refuse result on all
  900 fuzz cases; MPEG-2/2.5 and free-format MP3 are not recompressed by the encoder at all (stay raw), nothing to port.
  The Python mirrors (tests/mirror) are now only a historical reference; app_mirror.py still answers "unsupported" for MP3.
  NEXT: CP9 (optional): encoder / pack in Rust (needs kz_lz match finder, rc encoder, WAV/MP3 forward filters; ~72 KB of C in
  tools/accel_reference.c + the MP3 pack side, parse_file/mp3c_compress, which is NOT ported). Packing speed problems
  (see README.txt notes 1, 4, 7) were diagnosed separately: the app's pack pool silently falls back to one core (make_pool's bare
  `except Exception: return None` in kiln_v14.py); step 1-2 of that diagnosis (expose the error + show cores in the result sheet) is
  still open.
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
