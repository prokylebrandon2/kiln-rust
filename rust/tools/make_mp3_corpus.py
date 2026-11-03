#!/usr/bin/env python3
"""Build the MP3 test corpus for kiln-rs/tests/mp3_corpus.rs.

usage: make_mp3_corpus.py <kiln_v14.py> <outdir>
Needs ffmpeg (libmp3lame, libshine) and gcc (the real engine compiles its C helper).
For every case NAME it writes NAME.mp3 (the original), NAME.blob (the packed stream made by the REAL
engine's kz_mp3_pack) and checks that the real kz_mp3_unpack gives the original back.
"""
import importlib.util, os, random, struct, subprocess, sys, tempfile

kv, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
spec = importlib.util.spec_from_file_location("kiln_v14", kv)
K = importlib.util.module_from_spec(spec)
sys.modules["kiln_v14"] = K
spec.loader.exec_module(K)
K.load_accel() if hasattr(K, "load_accel") else None

SRC = {
    "sine": "sine=f=440:d={d}",
    "pink": "anoisesrc=d={d}:c=pink:a=0.5",
    "music": "aevalsrc='0.4*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*3*t))+0.25*sin(2*PI*330*t+sin(2*PI*5*t))+0.15*sin(2*PI*1760*t)':d={d}",
    "clicks": "aevalsrc='0.7*sin(2*PI*180*t)*lt(mod(t,0.45),0.05)+0.2*sin(2*PI*2500*t)*lt(mod(t,0.9),0.02)':d={d}",
    "sweep": "aevalsrc='0.6*sin(2*PI*(100*t+300*t*t))':d={d}",
}


def ffm(src, d, args, ext="mp3", stereo=True):
    with tempfile.TemporaryDirectory() as t:
        o = os.path.join(t, "o." + ext)
        s = SRC[src].format(d=d)
        cmd = ["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", s] + (["-af", "pan=stereo|c0=c0|c1=0.5*c0+0.5*sin(1)*c0"] if False else []) + args + [o]
        subprocess.run(cmd, check=True)
        return open(o, "rb").read()


cases = {}
def add(name, data):
    cases[name] = data

lame = ["-c:a", "libmp3lame"]
for src in SRC:
    add(f"{src}_cbr128", ffm(src, 12, lame + ["-b:a", "128k", "-ar", "44100", "-ac", "2"]))
add("music_mono64", ffm("music", 20, lame + ["-b:a", "64k", "-ar", "44100", "-ac", "1"]))
add("music_32k_48k", ffm("music", 20, lame + ["-b:a", "32k", "-ar", "32000", "-ac", "2"]))
add("music_48k_192", ffm("music", 10, lame + ["-b:a", "192k", "-ar", "48000", "-ac", "2"]))
add("music_320", ffm("music", 8, lame + ["-b:a", "320k", "-ar", "44100", "-ac", "2"]))
add("pink_320", ffm("pink", 6, lame + ["-b:a", "320k", "-ar", "44100", "-ac", "2"]))
add("pink_vbr0", ffm("pink", 8, lame + ["-q:a", "0", "-ar", "44100", "-ac", "2"]))
add("clicks_vbr4", ffm("clicks", 12, lame + ["-q:a", "4", "-ar", "44100", "-ac", "2"]))
add("music_vbr9", ffm("music", 30, lame + ["-q:a", "9", "-ar", "32000", "-ac", "1"]))
add("music_abr", ffm("music", 15, lame + ["-abr", "1", "-b:a", "96k", "-ar", "44100", "-ac", "2"]))
add("music_js0", ffm("music", 10, lame + ["-b:a", "128k", "-joint_stereo", "0", "-ar", "44100", "-ac", "2"]))
add("music_js1", ffm("music", 10, lame + ["-b:a", "128k", "-joint_stereo", "1", "-ar", "44100", "-ac", "2"]))
add("music_noreservoir", ffm("music", 10, lame + ["-b:a", "128k", "-reservoir", "0", "-ar", "44100", "-ac", "2"]))
add("music_tags", ffm("music", 10, lame + ["-b:a", "128k", "-ar", "44100", "-ac", "2", "-id3v2_version", "3", "-write_id3v1", "1",
                                          "-metadata", "title=Test", "-metadata", "artist=Kiln"]))
add("music_noxing", ffm("music", 10, lame + ["-b:a", "128k", "-ar", "44100", "-ac", "2", "-write_xing", "0"]))
for br in (64, 128, 256):
    add(f"shine_{br}", ffm("music", 10, ["-c:a", "libshine", "-b:a", f"{br}k", "-ar", "44100", "-ac", "2"]))
add("shine_mono", ffm("clicks", 10, ["-c:a", "libshine", "-b:a", "64k", "-ar", "44100", "-ac", "1"]))
add("shine_48k", ffm("sweep", 10, ["-c:a", "libshine", "-b:a", "96k", "-ar", "48000", "-ac", "2"]))
add("sweep_cbr160", ffm("sweep", 10, lame + ["-b:a", "160k", "-ar", "44100", "-ac", "2"]))

# derived variants (junk, truncation, concatenation, CRC flag)
base = cases["music_cbr128"]
rnd = random.Random(7)
add("trail_junk", base + bytes(rnd.randrange(256) for _ in range(777)))
add("lead_junk", bytes(rnd.randrange(256) for _ in range(1234)) + base)
add("trunc_mid_frame", base[:len(base) // 2 + 123])
add("concat_two", base + cases["music_mono64"])
add("concat_diff_sr", cases["music_32k_48k"] + cases["music_48k_192"])
mid = bytearray(base)
for _ in range(40):
    mid[rnd.randrange(len(mid) // 2, len(mid) - 1)] ^= 1 << rnd.randrange(8)   # a few damaged frames stay raw
add("bitflips", bytes(mid))
# CRC-protected-looking frames: clear the "no CRC" bit and put two arbitrary bytes after the header
# (shifts the frame content by 2 bytes; the parser keeps whatever it can decode and the rest raw)
def crcify(data):
    out = bytearray(); pos = 0
    BR = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320]; SR = [44100, 48000, 32000]
    while pos + 4 <= len(data):
        h = data[pos:pos + 4]
        if h[0] == 0xFF and (h[1] & 0xFE) == 0xFA and (h[2] >> 4) not in (0, 15) and ((h[2] >> 2) & 3) != 3:
            fs = 144000 * BR[h[2] >> 4] // SR[(h[2] >> 2) & 3] + ((h[2] >> 1) & 1)
            fr = bytearray(data[pos:pos + fs])
            if len(fr) == fs and fr[1] & 1:
                nb = bytearray(fr[:4]); nb[1] &= 0xFE
                nb += bytes([rnd.randrange(256), rnd.randrange(256)]) + fr[4:fs - 2]
                out += nb; pos += fs; continue
        out.append(data[pos]); pos += 1
    return bytes(out)
add("crc_flag", crcify(base))
add("crc_flag_mono", crcify(cases["music_mono64"]))

made = 0
for name, data in sorted(cases.items()):
    n = len(data)
    blob = K.mp3_pack(data)
    if not blob:
        print(f"{name}: not packable by the real engine (n={n}), skipped")
        continue
    assert K.mp3_unpack(blob, n) == data
    open(os.path.join(out, name + ".mp3"), "wb").write(data)
    open(os.path.join(out, name + ".blob"), "wb").write(blob)
    made += 1
    print(f"{name}: {n} -> {len(blob)} ({100.0 * len(blob) / n:.1f}%)")
print(made, "cases")
