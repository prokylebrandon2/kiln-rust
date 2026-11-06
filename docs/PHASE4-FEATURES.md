# Phase 4 feature report (part 4)

This report was written before the code was first compiled; everything here is now built on every commit and tested on a device by the maintainer.

## Done in this slice

| Feature | Where | Behaviour |
|---|---|---|
| Password-protected `.kiln` | `bridge/src/lock.rs` | A plain archive wrapped in a chunked, authenticated envelope: scrypt (N=2^17, r=8, p=1) key from the NFKC-normalised password, ChaCha20-Poly1305 per 1 MiB chunk, header authenticated, truncation detected. Same 8-byte magic and flag byte 12 as a plain archive, so the file browser shows the lock without changes; the plain engine refuses it as "unsupported". |
| Encrypt on pack | `packing.rs` | Engine packs a plain archive beside the destination, which is locked into place; temp files are removed on failure or cancel. Empty password is refused ("enter a password"); a plain archive is never written when a lock was asked for. |
| `need_password` / `bad_password` | `arc.rs` | `open_archive` returns `need_password:true` with no password and `bad_password:true` for a wrong one; extract, test and preview return the same flags. `encrypted:true` on a locked archive. |
| Fast listing | `lock.rs` | Opening a locked archive decrypts only its header, footer and index (the rest is a hole in the scratch copy). |
| Random-name sets | `sets.rs`, `packing.rs` | Pack each + encrypt writes `<32 hex>.kiln` files and a locked `_index.kiln` (JSON: random name, path, size, mtime). One key derivation for the whole set. `open_archive` on the folder returns `kind:"set"`. |
| Set extract / test / preview | `sets.rs` | Selected files are decrypted and extracted one at a time into a staging folder inside the destination, then moved into place; cancel removes only what the run created. |
| `decrypt_in_place` | `arc.rs`, `sets.rs` | A set becomes an ordinary folder (real names, archives and index removed only after every file came out). A locked archive file becomes a plain archive. |
| Settings off Python | `SettingsActivity.java` | WAV row and viewer removed; Chaquopy imports and `py()` gone; cache size/clear and tools go through `Engine`. |
| Engine temp folder | `Engine.java` | The bridge runs with `TMPDIR` set to `cache/engine-tmp`, so scratch copies are inside the app cache (covered by Settings > Cache). |

## Done in part 4

| Feature | Where | Behaviour |
|---|---|---|
| Editing sessions | `bridge/src/edit.rs` | `begin`, `state`, `close`, `discard`, `add`, `remove`, `transfer`, `set_file`, `new_text`, `file_for`, `save`. Session state lives on disk in `$TMPDIR/kiln-edit-<sid>/` (`session.json`, mode 0600, plus `staged/`); folders older than 24 h are swept at `begin`. `save` rebuilds the archive in scratch, packs it beside the original and renames it into place; locked archives are re-locked with the session password. |
| Tool: benchmark, self-test | `bridge/src/tool.rs` | `tool("bench", path)` and `tool("selftest", "")` return `{ok, text, view}` in the shapes `SettingsActivity` reads; cancel returns `{ok:false, cancelled:true}`. |

## Not done

Nothing remains from the Phase 4 list.

## Limits worth knowing

* Random-name sets cannot be edited ("Editing a random-name set isn't supported yet").
* `save` needs scratch space about the size of the archive, more for a locked one.
* `session.json` holds the archive password until the session closes.

* Extract, test and preview of a locked archive decrypt the whole archive into the scratch folder first, so they need free space equal to the archive size. Listing does not.
* Preview of one file from a locked archive therefore costs a full decrypt.
* Locked archives and sets use a new envelope, not the old app's `KLNA` format: old encrypted archives are not readable.
* Pack each with encrypt refuses a folder that already holds a set, even with overwrite.
* `-f` on a locked pack does not exclude an old destination that sits inside an input folder.

## Build notes

* `bridge/Cargo.toml` gains `chacha20poly1305 0.10`, `scrypt 0.11` (no default features) and `unicode-normalization 0.1`. `Cargo.lock` has been regenerated and includes them.
* README still says "No encryption"; left untouched on request.
