"""usage: fuzz_rc.py <kiln_v14.py> <corpus_dir> [count]: random corruptions; mirror must give the
same success/error text and the same bytes as the real engine."""
import importlib.util, json, os, random, sys
sys.path.insert(0, os.path.dirname(__file__))
import rc_mirror
spec = importlib.util.spec_from_file_location("kiln_v14", sys.argv[1]); K = importlib.util.module_from_spec(spec); spec.loader.exec_module(K)
corp = sys.argv[2]; count = int(sys.argv[3]) if len(sys.argv) > 3 else 200
TXT = {-1: "bad code", -2: "ran past its end", -3: "bad distance", -4: "too long", -5: "overread", -6: "size mismatch", -7: "out of memory"}
vecs = [v for v in json.load(open(os.path.join(corp, "expected.json")))["blocks"] if v["raw_len"] < 25000]
rnd = random.Random(7); bad = 0; kinds = {}
for t in range(count):
    v = rnd.choice(vecs); b = os.path.join(corp, "blocks", v["name"])
    pay = bytearray(open(b + ".payload", "rb").read()); hist = open(b + ".hist", "rb").read()
    mode = rnd.choice(["flip", "trunc", "flag", "rawlen"])
    raw = v["raw_len"]
    if mode == "flip":
        for _ in range(rnd.randint(1, 3)): pay[rnd.randrange(len(pay))] ^= 1 << rnd.randrange(8)
    elif mode == "trunc": pay = pay[:rnd.randrange(0, len(pay))]
    elif mode == "flag": pay[0] = rnd.randrange(256)
    else: raw = max(0, raw + rnd.choice([-5, -1, 1, 7, 1000]))
    pay = bytes(pay)
    h1 = bytearray(hist); h2 = bytearray(hist)
    try: K.decode_block(pay, raw, h1); r1 = "ok"
    except K.KilnError as e: r1 = str(e)
    try: rc_mirror.decode_block(pay, raw, h2); r2 = "ok"
    except ValueError as e: r2 = K._DECODE_ERRORS.get(e.args[0], "corrupt data: bad block")
    k = (mode, r1); kinds[k] = kinds.get(k, 0) + 1
    if r1 != r2 or h1 != h2:
        bad += 1; print("MISMATCH", v["name"], mode, r1, "|", r2, len(h1), len(h2))
print(f"{count - bad}/{count} same; outcomes:", {f'{m}:{r[-25:]}': n for (m, r), n in sorted(kinds.items())})
sys.exit(1 if bad else 0)
