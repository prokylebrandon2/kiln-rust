//! Executable branch filters (table filter ids 121 = x86, 122 = AArch64): port of `kz_bcj`
//! (decode direction) and of the python `Bcj` class (independent 1 MiB blocks).

pub const BCJ_X86: u16 = 121;
pub const BCJ_ARM64: u16 = 122;
pub const BCJ_BLOCK: usize = 1 << 20;

/// kz_bcj with dec = true. `pos0` = file offset of b[0]. Same length in and out.
pub fn bcj_decode(fid: u16, b: &mut [u8], pos0: u64) {
    let n = b.len();
    if fid == BCJ_X86 {
        let mut i = 0usize;
        while i + 4 < n {
            if (b[i] & 0xFE) == 0xE8 && (b[i + 4] == 0 || b[i + 4] == 0xFF) {
                let mut v = (b[i + 1] as u32)
                    | ((b[i + 2] as u32) << 8)
                    | ((b[i + 3] as u32) << 16)
                    | (((b[i + 4] & 1) as u32) << 24);
                let pc = (pos0.wrapping_add(i as u64 + 5)) as u32;
                v = v.wrapping_sub(pc) & 0x01FF_FFFF;
                b[i + 1] = v as u8;
                b[i + 2] = (v >> 8) as u8;
                b[i + 3] = (v >> 16) as u8;
                b[i + 4] = 0u32.wrapping_sub((v >> 24) & 1) as u8;
            }
            i += 1;
        }
    } else if fid == BCJ_ARM64 {
        let mut i = 0usize;
        while i + 4 <= n {
            let mut x = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            let pc = (pos0.wrapping_add(i as u64) >> 2) as u32;
            if (x & 0xFC00_0000) == 0x9400_0000 {
                let v = x.wrapping_sub(pc) & 0x03FF_FFFF;
                x = 0x9400_0000 | v;
            } else if (x & 0x9F00_0000) == 0x9000_0000 {
                let src = ((x >> 29) & 3) | ((x >> 3) & 0x001F_FFFC);
                if (src.wrapping_add(0x0002_0000) & 0x001C_0000) == 0 {
                    let q = (pos0.wrapping_add(i as u64) >> 12) as u32;
                    let d = src.wrapping_sub(q);
                    x &= 0x9000_001F;
                    x |= (d & 3) << 29;
                    x |= (d & 0x0003_FFFC) << 3;
                    x |= 0u32.wrapping_sub(d & 0x0002_0000) & 0x00E0_0000;
                }
            }
            b[i..i + 4].copy_from_slice(&x.to_le_bytes());
            i += 4;
        }
    }
}

/// Streaming decoder: buffers input and runs whole 1 MiB blocks; `finish` runs the tail.
pub struct Bcj {
    fid: u16,
    pos: u64,
    buf: Vec<u8>,
}

impl Bcj {
    pub fn new(fid: u16) -> Bcj {
        Bcj { fid, pos: 0, buf: Vec::new() }
    }
    fn run(&mut self, n: usize) -> Vec<u8> {
        let mut blk: Vec<u8> = self.buf.drain(..n).collect();
        bcj_decode(self.fid, &mut blk, self.pos);
        self.pos += n as u64;
        blk
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while self.buf.len() >= BCJ_BLOCK {
            out.extend_from_slice(&self.run(BCJ_BLOCK));
        }
        out
    }
    pub fn finish(&mut self) -> Vec<u8> {
        if self.buf.is_empty() {
            Vec::new()
        } else {
            let n = self.buf.len();
            self.run(n)
        }
    }
}

/// kz_bcj with dec = false (the packer side). `pos0` = file offset of b[0]. Same length in and out.
/// x86 runs right to left so every decision sees the same bytes the decoder will see.
pub fn bcj_encode(fid: u16, b: &mut [u8], pos0: u64) {
    let n = b.len();
    if fid == BCJ_X86 {
        if n < 5 {
            return;
        }
        let mut i = n - 5;
        loop {
            if (b[i] & 0xFE) == 0xE8 && (b[i + 4] == 0 || b[i + 4] == 0xFF) {
                let mut v = (b[i + 1] as u32)
                    | ((b[i + 2] as u32) << 8)
                    | ((b[i + 3] as u32) << 16)
                    | (((b[i + 4] & 1) as u32) << 24);
                let pc = (pos0.wrapping_add(i as u64 + 5)) as u32;
                v = v.wrapping_add(pc) & 0x01FF_FFFF;
                b[i + 1] = v as u8;
                b[i + 2] = (v >> 8) as u8;
                b[i + 3] = (v >> 16) as u8;
                b[i + 4] = 0u32.wrapping_sub((v >> 24) & 1) as u8;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
    } else if fid == BCJ_ARM64 {
        let mut i = 0usize;
        while i + 4 <= n {
            let mut x = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            let pc = (pos0.wrapping_add(i as u64) >> 2) as u32;
            if (x & 0xFC00_0000) == 0x9400_0000 {
                let v = x.wrapping_add(pc) & 0x03FF_FFFF;
                x = 0x9400_0000 | v;
            } else if (x & 0x9F00_0000) == 0x9000_0000 {
                let src = ((x >> 29) & 3) | ((x >> 3) & 0x001F_FFFC);
                if (src.wrapping_add(0x0002_0000) & 0x001C_0000) == 0 {
                    let q = (pos0.wrapping_add(i as u64) >> 12) as u32;
                    let d = src.wrapping_add(q);
                    x &= 0x9000_001F;
                    x |= (d & 3) << 29;
                    x |= (d & 0x0003_FFFC) << 3;
                    x |= 0u32.wrapping_sub(d & 0x0002_0000) & 0x00E0_0000;
                }
            }
            b[i..i + 4].copy_from_slice(&x.to_le_bytes());
            i += 4;
        }
    }
}

/// Streaming encoder: same 1 MiB independent blocks as the decoder, so `Bcj` undoes it exactly.
pub struct BcjEnc {
    fid: u16,
    pos: u64,
    buf: Vec<u8>,
}

impl BcjEnc {
    pub fn new(fid: u16) -> BcjEnc {
        BcjEnc { fid, pos: 0, buf: Vec::new() }
    }
    fn run(&mut self, n: usize) -> Vec<u8> {
        let mut blk: Vec<u8> = self.buf.drain(..n).collect();
        bcj_encode(self.fid, &mut blk, self.pos);
        self.pos += n as u64;
        blk
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while self.buf.len() >= BCJ_BLOCK {
            out.extend_from_slice(&self.run(BCJ_BLOCK));
        }
        out
    }
    pub fn finish(&mut self) -> Vec<u8> {
        if self.buf.is_empty() {
            Vec::new()
        } else {
            let n = self.buf.len();
            self.run(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<u8> {
        let mut s = 12345u32;
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            let r = (s >> 16) as u8;
            v.push(if i % 7 == 0 { 0xE8 } else if i % 11 == 4 { 0 } else if i % 13 == 0 { 0xFF } else { r });
        }
        v
    }

    #[test]
    fn roundtrip_both_filters() {
        for &fid in &[BCJ_X86, BCJ_ARM64] {
            for &n in &[0usize, 1, 4, 5, 6, 331, 4096, BCJ_BLOCK + 777] {
                let orig = sample(n);
                let mut e = BcjEnc::new(fid);
                let mut enc = e.feed(&orig);
                enc.extend(e.finish());
                assert_eq!(enc.len(), orig.len());
                let mut d = Bcj::new(fid);
                let mut dec = d.feed(&enc);
                dec.extend(d.finish());
                assert_eq!(dec, orig, "fid {fid} n {n}");
            }
        }
    }
}
