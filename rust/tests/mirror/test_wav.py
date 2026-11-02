"""usage: test_wav.py <kiln_v14.py> [layout|kernels|stream|all]
Mirror of kiln-rs/src/wav.rs vs the REAL engine (kiln_v14.Wav / wav_layout / C kernels)."""
import importlib.util, os, random, struct, sys, math
sys.path.insert(0, os.path.dirname(__file__))
import wav_mirror as M
spec = importlib.util.spec_from_file_location("kiln_v14", sys.argv[1]); K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
part = sys.argv[2] if len(sys.argv) > 2 else "all"
fails = 0
def check(name, ok, extra=""):
    global fails; print(("PASS " if ok else "FAIL ") + name + (" " + extra if extra else ""), flush=True); fails += (not ok)

def mk_wav(bits, tag, ch, frames, seed, from_int=0, ext=False, shift=0, extra_chunks=b"", trail=b"", dlen_over=None):
    rng = random.Random(seed); data = bytearray()
    for i in range(frames):
        base = math.sin(i * 0.031) * 0.4 + math.sin(i * 0.0071) * 0.2
        for c in range(ch):
            x = base * (1 - 0.1 * c) + rng.gauss(0, 0.004)
            if tag == 3 and from_int: data += struct.pack("<f", int(x * (1 << (from_int - 1))) / (1 << (from_int - 1)))
            elif tag == 3 and bits == 64: data += struct.pack("<d", x)
            elif tag == 3: data += struct.pack("<f", x)
            elif bits == 8: data.append(int(x * 127) + 128)
            else: data += (int(x * ((1 << (bits - 1 - shift)) - 1)) << shift).to_bytes(bits // 8, "little", signed=True)
    bps = ch * bits // 8
    if ext:
        fmt = (struct.pack("<IHHIIHH", 40, 0xFFFE, ch, 44100, 44100 * bps, bps, bits) + struct.pack("<HHI", 22, bits, 0) + struct.pack("<H", tag)
               + b"\x00\x00\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71")
        hdr = b"RIFF" + struct.pack("<I", 4 + 8 + 40 + 8 + len(data)) + b"WAVEfmt " + fmt
    else:
        hdr = (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, tag, ch, 44100, 44100 * bps, bps, bits))
    hdr += extra_chunks
    hdr += b"data" + struct.pack("<I", len(data) if dlen_over is None else dlen_over)
    return hdr + bytes(data) + trail

def real_layout(h, size):
    r = K.wav_layout(h, size)
    return M.NEED if r is K._NEED else M.NO if r is None else r

if part in ("layout", "all"):
    rng = random.Random(5); bad = 0; n = 0
    base = [mk_wav(16, 1, 2, 50, 1), mk_wav(24, 1, 1, 30, 2, ext=True), mk_wav(32, 3, 2, 40, 3), mk_wav(64, 3, 1, 20, 4), mk_wav(8, 1, 3, 17, 5),
            mk_wav(16, 1, 2, 50, 6, extra_chunks=b"LIST" + struct.pack("<I", 5) + b"abcde\0"), mk_wav(16, 1, 2, 50, 7, dlen_over=0xFFFFFFFF),
            mk_wav(16, 1, 2, 50, 8, trail=b"junk"), mk_wav(16, 1, 2, 50, 9, dlen_over=7)]
    for w in base:
        for cut in list(range(0, 70)) + [len(w) - 1, len(w)]:
            h = w[:cut]
            for size in (len(h), len(w), len(w) + 5, 10):
                n += 1
                if M.wav_layout(h, size) != real_layout(h, size): bad += 1
        for _ in range(300):                      # random corruption of the header bytes
            b = bytearray(w[:80]); b[rng.randrange(len(b))] = rng.getrandbits(8)
            for size in (len(w), 60, 1 << 20):
                n += 1
                if M.wav_layout(bytes(b), size) != real_layout(bytes(b), size): bad += 1
    check("wav_layout == real wav_layout", bad == 0, f"{n} cases, {bad} differ")

fids = [f for f in list(range(0, 12)) + list(range(16, 112)) if True]
if part in ("kernels", "all"):
    rng = random.Random(11); bad = 0; tested = 0; specdiff = 0
    for fid in range(0, 130):
        if (K.wav_spec(fid) is None) != (M.wav_spec(fid) is None) or (K.wav_spec(fid) is not None and tuple(K.wav_spec(fid)) != tuple(M.wav_spec(fid))): specdiff += 1
    check("wav_spec == real wav_spec for ids 0..129", specdiff == 0, f"{specdiff} differ")
    for fid in list(range(1, 12)) + list(range(16, 63)) + list(range(64, 111)):
        sp = K.wav_spec(fid)
        if sp is None: continue
        kind, W, order, ms, zz, bias, s = sp
        for C in ((2,) if ms else (1, 2, 3)):
            for n in (0, 1, 2, 331):
                if kind == "fint":
                    ks = [rng.randint(-(1 << s), (1 << s) - 1) for _ in range(n * C)]
                    data = struct.pack(f"<{n * C}f", *[k / (1 << s) for k in ks])
                else:
                    data = bytes(rng.getrandbits(8) for _ in range(n * C * W))
                f = K.Wav(fid, 1 << 40); f.C, f.align = C, C * W
                enc = f._enc(data); real = f._dec(enc)
                m = M.Wav(fid, 1 << 40); m.c, m.align = C, C * W
                mine = m.dec(enc)
                tested += 1
                if mine != real or real != data: bad += 1; print("  differs: fid", fid, "C", C, "n", n, flush=True)
    check("decode kernels: mirror == real C == original", bad == 0, f"{tested} filter/channel/length combos, {bad} differ")
    # arbitrary (non-roundtrip) filtered bytes: the decoders must agree on ANY input
    bad = 0; tested = 0
    for fid in list(range(1, 12)) + list(range(16, 63, 3)) + list(range(64, 111, 3)):
        sp = K.wav_spec(fid)
        if sp is None: continue
        kind, W, order, ms, zz, bias, s = sp
        kw = (s + 1 + 7) // 8 if kind == "fint" else W
        for C in ((2,) if ms else (1, 3)):
            n = 97
            enc = bytes(rng.getrandbits(8) for _ in range(n * C * (2 if kind == "w16" else W)))
            f = K.Wav(fid, 1 << 40); f.C, f.align = C, C * W
            m = M.Wav(fid, 1 << 40); m.c, m.align = C, C * W
            tested += 1
            if f._dec(enc) != m.dec(enc): bad += 1; print("  differs (random input): fid", fid, "C", C, flush=True)
    check("decode kernels on arbitrary bytes (incl. NaN/inf floats)", bad == 0, f"{tested} combos, {bad} differ")

if part in ("stream", "all"):
    cases = [("pcm16", mk_wav(16, 1, 2, 3000, 1)), ("pcm16_mono", mk_wav(16, 1, 1, 2500, 2)), ("pcm24", mk_wav(24, 1, 2, 2000, 3)),
             ("pcm32", mk_wav(32, 1, 2, 1500, 4)), ("pcm8", mk_wav(8, 1, 2, 2200, 5)), ("pcm24_ext_mono", mk_wav(24, 1, 1, 2100, 6, ext=True)),
             ("float32", mk_wav(32, 3, 2, 1800, 7)), ("float64", mk_wav(64, 3, 2, 900, 8)), ("float_from16", mk_wav(32, 3, 2, 2000, 9, from_int=16)),
             ("float_from24_ext", mk_wav(32, 3, 2, 1700, 10, from_int=24, ext=True)), ("float_from8_mono", mk_wav(32, 3, 1, 2300, 11, from_int=8)),
             ("pcm16_tail", mk_wav(16, 1, 2, 1999, 12, trail=b"TAILBYTES1")), ("pcm16_oddtail", mk_wav(16, 1, 2, 1000, 13, trail=b"x")),
             ("pcm16_list", mk_wav(16, 1, 2, 1500, 14, extra_chunks=b"LIST" + struct.pack("<I", 5) + b"abcde\0")), ("pcm16_6ch", mk_wav(16, 1, 6, 900, 15)),
             ("pcm16_big_dlen", mk_wav(16, 1, 2, 1200, 16, dlen_over=0xFFFFFFFF)), ("pcm16_empty", mk_wav(16, 1, 2, 0, 17)), ("pcm24in32", mk_wav(32, 1, 2, 1100, 18, shift=8))]
    for frames in (1 << 18, 700, 64):             # 700 / 64 frames per chunk exercise the chunk boundaries and tails
        K.WAV_FRAMES = M.WAV_FRAMES = frames
        bad = 0; tested = 0; fidsused = set()
        for name, w in cases:
            size = len(w)
            fid = K.pick_wav_filter_bytes(w) if hasattr(K, "pick_wav_filter_bytes") else None
            cands = []
            lay = K.wav_layout(w[:K.WAV_HEAD], size)
            if lay is None or lay is K._NEED: cands = []
            else:
                _s, _r, ch, al, bps, fl = lay
                for f in range(1, 111):
                    sp = K.wav_spec(f)
                    if sp is None or sp[1] != bps and not (sp[0] == "fint" and bps == 4): continue
                    if sp[0] == "w16" and bps != 2: continue
                    if sp[0] == "fint" and not fl: continue
                    if sp[0] in ("int", "w16") and fl: continue
                    if sp[3] and ch != 2: continue
                    cands.append(f)
                rng2 = random.Random(len(name)); rng2.shuffle(cands); cands = cands[:10]
            for f in cands:
                try:
                    enc = K.Wav(f, size)
                    filt = enc.feed(w) + enc.finish()
                except K.KilnError:
                    continue                      # not convertible (float not integer audio etc.)
                assert len(filt) == size
                # decode in awkward piece sizes with real and mirror
                for piece in (1, 997, 65537, size):
                    if piece == 1 and size > 6000: continue
                    r = K.Wav(f, size, True); m = M.Wav(f, size); ro = bytearray(); mo = bytearray()
                    for i in range(0, size, piece):
                        ro += r.feed(filt[i:i + piece]); mo += m.feed(filt[i:i + piece])
                    ro += r.finish(); mo += m.finish()
                    tested += 1; fidsused.add(f)
                    if bytes(mo) != bytes(ro) or bytes(ro) != w: bad += 1; print("  differs:", name, "fid", f, "piece", piece, "frames", frames, flush=True)
        check(f"stream decode, WAV_FRAMES={frames}: mirror == real == original", bad == 0 and tested > 100, f"{tested} runs, {len(fidsused)} distinct filters, {bad} differ")
    K.WAV_FRAMES = M.WAV_FRAMES = 1 << 18
    # error paths: wrong header for the chosen filter / garbage header
    bad = 0; n = 0
    w = mk_wav(16, 1, 2, 500, 1); size = len(w)
    for f, blob in ((9, w), (1, mk_wav(24, 1, 2, 500, 2)), (5, mk_wav(16, 1, 1, 500, 3)), (16, b"not a wav file at all, but long enough to decide" * 5), (1, b"RIFF")):
        r = K.Wav(f, len(blob), True); m = M.Wav(f, len(blob))
        def run(o):
            try: return ("ok", o.feed(blob) + o.finish())
            except Exception as e: return ("err", str(e))
        a, b = run(r), run(m); n += 1
        if a != b: bad += 1; print("  differs on error path: fid", f, a[0], b[0], a[1] if a[0] == "err" else "", flush=True)
    for fid in (0, 12, 13, 63, 111, 200):
        try: K.Wav(fid, 10, True); a = "ok"
        except K.KilnError as e: a = str(e)
        try: M.Wav(fid, 10); b = "ok"
        except M.KilnError as e: b = str(e)
        n += 1
        if a != b: bad += 1
    check("error paths and unknown ids: same results/messages", bad == 0, f"{n} cases, {bad} differ")
sys.exit(1 if fails else 0)
