"""Mutation test of inflate_model.py: each entry in M breaks the model in one place; the model's own
test suite (--quick) must then fail. A mutant that SURVIVES is a gap in the tests (or an equivalent
change: say so in the entry). Usage: python3 inflate_mutants.py [first last]  (default: all)."""
import subprocess, sys, os, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(HERE, "inflate_model.py")).read()
M = [
 ("copy_fast: 16-byte pieces from distance 15",
  '    if distance >= 16:\n        cov("fast match, distance >= 16")', '    if distance >= 15:\n        cov("fast match, distance >= 16")'),
 ("copy_fast: 8-byte pieces from distance 4",
  '    elif distance >= 8:\n        cov("fast match, distance 8..15")', '    elif distance >= 4:\n        cov("fast match, distance 8..15")'),
 ("FAST_OUT margin too small (260)", "FAST_OUT = 288", "FAST_OUT = 260"),
 ("refill: advance pos by (64-cnt)>>3", "self.pos += (63 - self.cnt) >> 3", "self.pos += (64 - self.cnt) >> 3"),
 ("sub_entry: index mask one bit too wide",
  "index = (e >> 16) + (b.buf & ((1 << ((e >> 8) & 15)) - 1))", "index = (e >> 16) + (b.buf & ((1 << (((e >> 8) & 15) + 1)) - 1))"),
 ("extra_value: shift by one too many",
  "return (saved & ((1 << (e & 0xFF)) - 1)) >> ((e >> 8) & 15)", "return (saved & ((1 << (e & 0xFF)) - 1)) >> (((e >> 8) & 15) + 1)"),
 ("build_table: subtable sized by the first code, not the longest",
  "sub_bits = lens[sorted_[end - 1]] - root", "sub_bits = lens[sorted_[k]] - root"),
 ("build_table: subtable index not shifted by root bits",
  "            i = r >> root\n", "            i = r\n"),
 ("stored block: forget to give buffered bytes back",
  "    bits.pos -= bits.cnt >> 3\n", "    pass\n"),
 ("overrun never reports", "return self.pos * 8 > len(self.input) * 8 + self.cnt", "return False"),
 ("slow loop: output-full check off by one", "if length > out_len - op:", "if length > out_len - op + 1:"),
 ("copy_exact: wide pieces with only 1 byte of room",
  "len(out) - op >= length + 16:", "len(out) - op >= length + 1:"),
 ("fast loop: third literal stored one byte too low",
  "put(out, op + 2, (e3 >> 16) & 0xFF)", "put(out, op + 1, (e3 >> 16) & 0xFF)"),
 ("fast loop: second literal's bits not consumed",
  "            b.skip(e2 & 0xFF)\n", "            pass\n"),
 ("build_table: every incomplete code accepted", "trivial = used == 0 or (used == 1 and count[1] == 1)", "trivial = True"),
 ("code length repeat: off-by-one at the end", "if i + repeat > total:", "if i + repeat >= total:"),
 ("fast loop: bad distance symbol not rejected",
  '        if d & F_BAD:\n            cov("fast loop: bad distance symbol")\n            raise InflateError("BadSymbol")',
  '        if False:\n            raise InflateError("BadSymbol")'),
 ("distance check off by one", "        if distance > op:\n            cov(\"fast loop: distance too far\")", "        if distance > op + 1:\n            cov(\"fast loop: distance too far\")"),
 ("dynamic header: one code length code too few", "        for sym in PRE_ORDER[:hclen]:\n            bits.refill()", "        for sym in PRE_ORDER[:hclen - 1]:\n            bits.refill()"),
 ("end of block entry loses its flag", "return F_EOB | head", "return head"),
 ("dist table: single-codeword code rejected",
  "build_table(lens[hlit:total], self.dist, DIST_ROOT, dist_entry, True)", "build_table(lens[hlit:total], self.dist, DIST_ROOT, dist_entry, False)"),
 ("subtable pointer forgets the root bit count",
  "table[prefix] = F_SUB | (next_sub << 16) | (sub_bits << 8) | root", "table[prefix] = F_SUB | (next_sub << 16) | (sub_bits << 8)"),
 ("copy_overlapping: pattern not doubled correctly",
  "n = min(length - done, distance + done)", "n = min(length - done, distance + done + 1)"),
 ("fast loop: fast-loop entry ignores input margin", "while b.pos + FAST_IN <= in_len and op + FAST_OUT <= out_len:", "while op + FAST_OUT <= out_len:"),
]

import sys as _sys
lo, hi = (int(_sys.argv[1]), int(_sys.argv[2])) if len(_sys.argv) > 2 else (0, len(M))
def one(item):
    name, old, new = item
    if src.count(old) != 1:
        return f"!! anchor for '{name}' matches {src.count(old)} times"
    mut = src.replace(old, new)
    f = tempfile.NamedTemporaryFile("w", suffix=".py", delete=False, dir=HERE)
    f.write(mut); f.close()
    try:
        r = subprocess.run([sys.executable, f.name, "--quick"], capture_output=True, text=True, timeout=25)
        out = r.stdout + r.stderr
        status = "KILLED  " if r.returncode != 0 else "SURVIVED"
        first = next((l for l in out.splitlines() if l.startswith("FAIL") or "Error" in l or "Traceback" in l), "")[:100]
    except subprocess.TimeoutExpired:
        status, first = "KILLED  ", "(timeout: the mutant hangs)"
    finally:
        os.unlink(f.name)
    return f"{status} {name}   {first}"
survivors = []
with open(os.path.join(HERE, "mutants.txt"), "w") as log:
    for i, item in enumerate(M[lo:hi], start=lo):
        line = f"[{i:2d}] " + one(item)
        print(line, flush=True)
        log.write(line + "\n")
        if "SURVIVED" in line or "!!" in line:
            survivors.append(line)
print(f"{min(hi, len(M)) - lo} mutants run, {len(survivors)} survived or misconfigured")
sys.exit(1 if survivors else 0)
