#!/usr/bin/env python3
"""Build tests/lzvec/ with the REAL kz_lz: input buffers, chunk classes (kz_rnd), and the exact
token stream + raw runs for several parameter sets. Used by kiln-rs/tests/lz_vectors.rs.
usage: make_lz_vectors.py <kiln_v14.py> <out_dir>"""
import importlib.util, json, os, random, struct, sys, array, ctypes as ct
src, out = sys.argv[1:3]
spec = importlib.util.spec_from_file_location("kiln_v14", src)
K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
random.seed(4242)
os.makedirs(out, exist_ok=True)
words = ["".join(random.choice("abcdefghijklmnop") for _ in range(random.randint(3, 9))) for _ in range(3000)]
rb = lambda n: bytes(random.getrandbits(8) for _ in range(n))
blob = rb(5000)
data = {
    "text": " ".join(random.choice(words) for _ in range(60000)).encode(),
    "code": open(src, "rb").read()[:300000],
    "floats": b"".join(struct.pack("<f", i * 0.25) for i in range(60000)),
    "records": b"".join(struct.pack("<IHHf", i, i % 7, (i * 3) & 0xFFFF, i / 3) for i in range(25000)),
    "rand_blob": rb(150000) + blob + rb(70000) + blob + rb(1000),
    "noisy": bytes((random.getrandbits(8) & 0xF0) | (i & 3) for i in range(200000)),
    "mixed": b"".join(random.choice([b"abc" * random.randint(1, 40), rb(random.randint(1, 60))]) for _ in range(2500)),
    "zeros": bytes(150000) + b"x" + bytes(5000),
    "tiny": b"hello hello hello",
    "empty1": b"a",
    "ls": open("/bin/ls", "rb").read()[:400000],
    "alpha64": bytes(random.choice(range(64)) for _ in range(330000)),
    "alpha16": bytes(random.choice(range(16)) for _ in range(200000)) + blob + bytes(random.choice(range(16)) for _ in range(70000)),
    "sparse": bytes((random.getrandbits(8) if random.random() < 0.10 else 0) for _ in range(300000)),
    "skew": bytes(random.choice(b"aaaaaaaabbbc") for _ in range(250000)),
    "noise_then_text": rb(100000) + " ".join(random.choice(words) for _ in range(30000)).encode() + bytes(random.choice(range(40)) for _ in range(140000)),
}
params = [(384, 258, 2, 99), (32, 64, 1, 4), (8, 32, 0, 2), (4096, 258, 2, 99), (384, 128, 2, 5)]
meta = []
for name, buf in data.items():
    n = len(buf)
    rnd = K.random_chunks(buf)
    rndb = bytes(1 if x == 1 else x for x in rnd)
    open(f"{out}/{name}.buf", "wb").write(buf)
    open(f"{out}/{name}.rnd", "wb").write(bytes(rnd))
    starts = [0] + ([min(n // 3, 70000)] if n > 4000 else [])
    for start in starts:
        for pi, (chain, nice, lazy, accel) in enumerate(params):
            for lp in ((0, 1, 2) if lazy >= 2 and pi == 0 else (0,)):
                cap = n - start + 16
                runcap = cap // 64 + 16
                tok = array.array("Q", [0]) * cap
                runs = array.array("q", [0]) * (2 * runcap)
                nr = ct.c_int64(0)
                nt = K._A.kz_lz(buf, n, start, bytes(rnd), len(rnd), chain, nice, lazy, accel,
                                K._addr(tok), K._addr(runs), runcap, ct.byref(nr), lp)
                assert nt >= 0
                tag = f"{name}_s{start}_p{pi}_l{lp}"
                open(f"{out}/{tag}.tok", "wb").write(tok[:nt].tobytes())
                open(f"{out}/{tag}.runs", "wb").write(runs[:2 * nr.value].tobytes())
                meta.append(dict(tag=tag, name=name, start=start, chain=chain, nice=nice, lazy=lazy,
                                 accel=accel, lp=lp, nt=nt, nr=nr.value))
# fuzz: many small odd inputs, random history offsets and parameters
for fi in range(150):
    alpha = random.choice([2, 3, 4, 16, 256])
    n = random.choice([1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 64, 100, 333, 1000, 4097, 9000])
    if random.random() < 0.5:
        pieces, buf = [], b""
        while len(buf) < n:
            if buf and random.random() < 0.5:
                a = random.randrange(len(buf)); buf += buf[a:a + random.randint(1, 80)]
            else:
                buf += bytes(random.randrange(alpha) for _ in range(random.randint(1, 30)))
        buf = buf[:n]
    else:
        buf = bytes(random.randrange(alpha) for _ in range(n))
    name = f"fz{fi}"
    rnd = K.random_chunks(buf)
    open(f"{out}/{name}.buf", "wb").write(buf)
    open(f"{out}/{name}.rnd", "wb").write(bytes(rnd))
    start = random.choice([0, 0, random.randrange(n + 1)])
    chain, nice, lazy, accel = random.choice(params)
    lp = random.choice((0, 1, 2))
    cap = n - start + 16
    runcap = cap // 64 + 16
    tok = array.array("Q", [0]) * cap
    runs = array.array("q", [0]) * (2 * runcap)
    nr = ct.c_int64(0)
    nt = K._A.kz_lz(buf, n, start, bytes(rnd), len(rnd), chain, nice, lazy, accel,
                    K._addr(tok), K._addr(runs), runcap, ct.byref(nr), lp)
    assert nt >= 0
    tag = f"{name}_s{start}_p0_l{lp}"
    open(f"{out}/{tag}.tok", "wb").write(tok[:nt].tobytes())
    open(f"{out}/{tag}.runs", "wb").write(runs[:2 * nr.value].tobytes())
    meta.append(dict(tag=tag, name=name, start=start, chain=chain, nice=nice, lazy=lazy,
                     accel=accel, lp=lp, nt=nt, nr=nr.value))
json.dump(meta, open(f"{out}/meta.json", "w"))
print(len(meta), "cases")
