#!/usr/bin/env python3
"""Build tests/encvec/ with the REAL kiln engine: token blocks (from the real C match finder) and
the payload the real C kz_rc_enc makes for each coding mode. Used by kiln-rs/tests/enc_vectors.rs.
usage: make_enc_vectors.py <kiln_v14.py> <out_dir>"""
import importlib.util, json, os, random, struct, sys
src, out = sys.argv[1:3]
spec = importlib.util.spec_from_file_location("kiln_v14", src)
K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
random.seed(77)
os.makedirs(out, exist_ok=True)
words = ["".join(random.choice("abcdefghijklmnop") for _ in range(random.randint(3, 9))) for _ in range(2500)]
data = {
    "text": " ".join(random.choice(words) for _ in range(30000)).encode(),
    "code": open(src, "rb").read()[:200000],
    "floats": b"".join(struct.pack("<f", i * 0.25) for i in range(30000)),
    "records": b"".join(struct.pack("<IHHf", i, i % 7, (i * 3) & 0xFFFF, i / 3) for i in range(12000)),
    "rand": bytes(random.getrandbits(8) for _ in range(30000)),
    "mixed": b"".join(random.choice([b"abc" * random.randint(1, 40), bytes(random.getrandbits(8) for _ in range(random.randint(1, 60)))]) for _ in range(900)),
    "zeros": bytes(70000) + b"x" + bytes(5000),
    "tiny": b"hello hello hello",
    "ls": open("/bin/ls", "rb").read(),
}
meta = []
for name, buf in data.items():
    for start in (0, 1000 if len(buf) > 2000 else 0):
        rnd = [1 if x == 1 else 0 for x in K.random_chunks(buf)]
        for lpm in (0, 1):
            parts = K.lz77(buf, start, rnd, lpm)
            pos = start
            for pi, part in enumerate(parts):
                if isinstance(part, int):
                    pos += part
                    continue
                # split big parts so a vector stays small, like BLOCK_TOKENS does at full size
                step = 40000
                for i in range(0, len(part), step):
                    tk = part[i:i + step]
                    for mode in K._MODES:
                        raw, payload = K.encode_block(tk, buf, pos, mode)
                        tag = f"{name}_s{start}_l{lpm}_p{pi}_{i}_m{mode}"
                        open(f"{out}/{tag}.tok", "wb").write(tk.tobytes())
                        meta.append(dict(tag=tag, name=name, pos=pos, mode=mode, raw=raw,
                                         ok=payload is not None))
                        if payload is not None:
                            open(f"{out}/{tag}.pay", "wb").write(payload)
                    pos += K.encode_block(tk, buf, pos, 0)[0]
for name, buf in data.items():
    open(f"{out}/{name}.buf", "wb").write(buf)
json.dump(meta, open(f"{out}/meta.json", "w"))
print(len(meta), "vectors")
