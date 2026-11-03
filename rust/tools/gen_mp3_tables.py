import re,sys
# usage: gen_mp3_tables.py <kiln_v14.py> <out.rs>
full=open(sys.argv[1],encoding='utf-8').read()
src=full[full.index('_MP3_SRC = r"""'):]
src=src[:src.index('"""',20)]
out=["//! Generated from kiln_v14.py (_MP3_SRC) by tools/gen_mp3_tables.py: MP3 Huffman tables (ISO 11172-3).",
     "//! Do not edit by hand.",""]
names=[]
for m in re.finditer(r'static const (uint16_t|uint8_t) (H[CL]_\d+)\[(\d+)\]=\{([^}]*)\};',src):
    ty,name,n,body=m.groups()
    rt='u16' if ty=='uint16_t' else 'u8'
    vals=[v for v in body.split(',') if v.strip()]
    assert len(vals)==int(n),(name,len(vals),n)
    out.append(f"pub static {name}: [{rt}; {n}] = [{','.join(v.strip() for v in vals)}];")
    names.append(name)
ht=re.search(r'static const HuffTab HT\[34\]=\{(.*?)\n\};',src,re.S).group(1)
rows=re.findall(r'\{(\d+),(\d+),(\w+),(\w+)\}',ht)
assert len(rows)==34
out.append("")
out.append("/// (xlen, linbits, codes, lengths); None = no table")
out.append("pub static HT: [(usize, u32, Option<&'static [u16]>, Option<&'static [u8]>); 34] = [")
for xl,lb,c,l in rows:
    if c=='0': out.append("    (0, 0, None, None),")
    else: out.append(f"    ({xl}, {lb}, Some(&{c}), Some(&{l})),")
out.append("];")
open(sys.argv[2],'w').write("\n".join(out)+"\n")
print(len(names),"tables")
