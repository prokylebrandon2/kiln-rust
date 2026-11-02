# Kiln .kln format (archive id "KLNA", engine v14) - decoder-side spec

Source of truth: app/src/main/python/kiln_v14.py (line numbers = v14 in project 4.0.5) and the
C source in `_ACCEL_SRC` (copy: tools/accel_reference.c). All integers big-endian unless noted.

## 1. File layout
    header | data stream | table stream | footer(20)
- header: b"KLNA" + u8 flags. flags: bit0 = encrypted; any other bit -> "newer kiln" error.
  Encrypted: + u8 log2N, u8 r, u8 p, 16-byte salt  (total 24 bytes). Limits: 10<=log2N<=20,
  1<=r<=16, 1<=p<=4. hdr_len = 5 or 24. "KLN2" magic = old format, unsupported.
- footer (last 20 bytes): u64 table_off | u64 total | b"KLNA".
  Valid when hdr_len <= table_off <= size-20. Plain archives: sum(entry.size) must equal total.
  (Encrypted: total not checked.)
- data stream starts at hdr_len (or a checkpoint offset) and ends with an end block.
- table stream starts at table_off, same record format, ends with an end block.
  Python only checks that the data stream ends exactly at table_off.

## 2. Records / blocks (plain)
    u8 type
      0 stored : u32 raw_len | u32 comp_len (must == raw_len) | payload
      1 range  : u32 raw_len | u32 comp_len | payload(comp_len bytes)
      2 end    : (no other bytes)
  comp_len > 1<<28 -> corrupt. raw_len for type 1 must be <= 524288*258.
  Stream output = concatenation of block outputs. Match distances may reach back into earlier
  blocks (history is kept up to 64 MB = WINDOW (1<<26)-1); coder MODELS restart each block.
  At a checkpoint the encoder forgets history, so decoding may start there with empty history.

## 3. Encrypted records
Every record (block or end marker) is: u32 len | AES-256-GCM(ciphertext||16-byte tag). len>=17.
- plaintext = exactly the plain record bytes (type|raw|comp|payload), or the single byte 0x02.
- nonce (12 bytes) = [stream,0,0,0] + u64 counter. stream 0 = data, 1 = table. Counter starts at 0
  for each stream (data stream decoding from a checkpoint starts at the checkpoint's counter).
- AAD = the whole header (24 bytes) + u8 is_final (1 only for the end marker record).
- decrypt order (kiln_v14.py CryptSource.read_record): try AAD+0; if OK and pt[0]==2 -> fail;
  on tag failure try AAD+1 and require pt == b"\x02", else fail ("authentication failed").
- key = scrypt(NFKC(password).utf8, salt, N=2^log2N, r, p, dklen=32).
- Python adds: 128 MiB scrypt memory at default settings (log2N 17, r 8).

## 4. Range-coded block payload (type 1) - see accel_reference.c kz_rc_dec (line ~761)
payload[0]: bits0-1 = literal position bits lp (0..2), bit2 = literal mixer on; bits above must
be 0 else error -1. payload[1..4] initial code bytes (code = 4 bytes from index 1, range=0xFFFFFFFF).
Reading past the payload end yields 0 bytes; error -2 if p > plen+8 while decoding; error -5 if
p > plen at the end.
Errors: -1 bad code, -2 ran past end, -3 bad distance, -4 block too long, -5 overread,
-6 size mismatch, -7 oom. (message texts: _DECODE_ERRORS, kiln_v14.py line 876.)
Model contexts, probability counters (a,b,n with rates tables RCR_A/RCR_B from RC_K=1.6, RA=4,
RB=7), the mixer tables (MIXSTR/MIXSQ/mix_sq) and weights init are all in the C file: port them
EXACTLY (including uint16 wraparound in rc_up, int32 weights, int64 dot product >>16).

## 5. File table (after decoding the table stream into bytes)
    u32 count
    per entry: u16 pathlen | path(utf-8) | u64 size | u32 crc32 | u64 mtime | u16 modefilt
       mode = modefilt & 0xFFF ; filt = modefilt >> 12
       if filt == 15: u16 v follows: dead = v&0x8000, filt = v&0x7FFF
            (filt<=11 with !dead, or unknown filt -> "features from a newer kiln")
       elif filt > 11: newer-kiln error
       if filt == 120 (MP3): u64 slen follows (stream length of the packed form), else slen = size
    optional trailer: b"CKPT" | u32 n | n * (u32 file_index | u64 stream_offset | u64 counter)
       constraints: 0 < idx < count; offsets strictly increasing (first > hdr_len-1); counters
       non-decreasing; offset < table_off.
  Filters: 1-11 WAV family (wav_spec), 120 MP3 repack, 121 BCJ x86, 122 BCJ AArch64.
  dead=true entries: data still in the stream, no unfilter, no CRC check, not listed to users.

## 6. Decoding files out of the data stream
Entries consume the stream in order, each exactly slen bytes. Entries with slen==0 need crc==0.
Per entry: bytes -> unfilter (if filt != 0 and not dead) -> crc32 (zlib, over ORIGINAL bytes) must
equal entry.crc ("checksum failed for <rel>"). Stream longer than table = corrupt; shorter =
truncated. BCJ: filter applied in independent 1 MiB blocks (BCJ_BLOCK), pos0 = byte offset in file
at block start, decode left->right for x86 (121), see kz_bcj in the C file (line 55).

## 7. Sets (folder of archives)
- `_index.kln`: an ENCRYPTED archive holding one file `index.json`:
  {"v":2,"files":{archive_name:{"path":orig_path,"size":int,"mtime":int}}} (v1: name->path str).
- each other archive in the folder is a normal .kln (usually encrypted with the same password).
- Extract/test of a set = read the index, then extract/test each listed archive in order.

## 8. Status of what Rust must cover (decoder): sections 1-6 now; 7 in CP8; WAV/MP3 unfilters CP8.
