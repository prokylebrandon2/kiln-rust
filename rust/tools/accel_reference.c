/* kiln_accel.c - optional C speed-ups for kiln.py (loaded through ctypes).
 *
 * kiln.py compiles this file the first time it is needed (cc/gcc/clang) and
 * stops with an error if that isn't possible: kiln has no pure-Python fallback.
 *
 *   kz_lz       LZ77 match finder (hash chains, lazy matching, chunk classes)
 *   kz_rc_enc/dec  adaptive range coder + LZ77 block decoder (bounds-checked)
 *   kz_wav16_*  WAV filter for 16-bit PCM (prediction + byte planes)
 *   kz_shuf_*   WAV byte planes (24/32/64-bit audio)
 *   kz_int_*    WAV integer predictor for 8/24/32-bit audio; kz_f2i/kz_i2f float-as-integer
 *   kz_rnd      chunk classifier (random / literal-only / sparse / normal)
 */
#include <stdint.h>
#include <stdlib.h>
#include <stddef.h>
#include <string.h>

#define KZ_VERSION 16

#define WINDOW ((1 << 26) - 1)
#define HIST_NEAR (1 << 20)
#define HIST_FAR_STRIDE 8
#define DBITS 32
#define DMASK ((((uint64_t)1) << DBITS) - 1)
#define HBITS 22
#define H3BITS 18           /* separate table: the nearest 3-byte match */
#define LBITS 22            /* long-match table: 8-byte hashes at every 4th position */
#define LONG_MIN 24
#define RND_BITS 16
#define MIN_MATCH 3
/* length-slot context: len 2 and 3 share one, then 4, 5, 6+ (v = len - 2) */
#define SLOTCTX(v) ((v) < 1 ? 0u : (v) < 4 ? (v) - 1 : 3u)
#define MAX_MATCH 258
#define DRY_CHAIN 8
#define SKIP_INSERT 192
#define LAZY_SKIP 64
#define MAX_STEP 32
#define MIN_RND_MATCH 24
#define SPARSE_MATCH 12
#define HIST_STRIDE 2
#define H6BITS 22
#define R6BITS 24          /* tree finder: the last 2^24 positions (a ring of tree nodes) */
#define R6MASK ((1 << R6BITS) - 1)
#define BTMODE 2
#define BTK 3
#define PRB_CHAIN 25
#define PRB_OPT 24
#define HISTD 4

/* ------------------------------------------------------------ executable filters
 * Same-length, exactly invertible transforms that turn relative branch targets into absolute
 * ones, so repeated calls to one function become repeated bytes. kz_bcj returns how many bytes
 * from the start are final (always n here: callers hand over whole independent blocks).
 * fid 121: x86 E8/E9 (call/jmp rel32), 122: AArch64 BL and ADRP. pos0 = file offset of b[0]. */
int64_t kz_bcj(int fid, uint8_t *b, int64_t n, int64_t pos0, int dec) {
    if (fid == 121) {
        /* encode right to left, decode left to right: each decision then sees the same bytes
         * on both sides even where call opcodes overlap an earlier operand */
        int64_t i = dec ? 0 : n - 5, step = dec ? 1 : -1;
        for (; i >= 0 && i + 4 < n; i += step) {
            if ((b[i] & 0xFE) == 0xE8 && (b[i + 4] == 0 || b[i + 4] == 0xFF)) {
                uint32_t v = (uint32_t)b[i + 1] | ((uint32_t)b[i + 2] << 8) | ((uint32_t)b[i + 3] << 16) | ((uint32_t)(b[i + 4] & 1) << 24);
                uint32_t pc = (uint32_t)(pos0 + i + 5);
                v = (dec ? v - pc : v + pc) & 0x01FFFFFFu;
                b[i + 1] = (uint8_t)v; b[i + 2] = (uint8_t)(v >> 8); b[i + 3] = (uint8_t)(v >> 16);
                b[i + 4] = (uint8_t)(0u - ((v >> 24) & 1));
            }
        }
        return n;
    }
    if (fid == 122) {
        int64_t i = 0;
        for (; i + 4 <= n; i += 4) {
            uint32_t x; memcpy(&x, b + i, 4);
            uint32_t pc = (uint32_t)((pos0 + i) >> 2);
            if ((x & 0xFC000000u) == 0x94000000u) {
                uint32_t v = (dec ? (x - pc) : (x + pc)) & 0x03FFFFFFu;
                x = 0x94000000u | v;
            } else if ((x & 0x9F000000u) == 0x90000000u) {
                uint32_t src = ((x >> 29) & 3u) | ((x >> 3) & 0x001FFFFCu);
                if (((src + 0x00020000u) & 0x001C0000u) == 0) {
                    uint32_t q = (uint32_t)((pos0 + i) >> 12);
                    uint32_t d = dec ? src - q : src + q;
                    x &= 0x9000001Fu;
                    x |= (d & 3u) << 29;
                    x |= (d & 0x0003FFFCu) << 3;
                    x |= (0u - (d & 0x00020000u)) & 0x00E00000u;
                }
            }
            memcpy(b + i, &x, 4);
        }
        return i;
    }
    return n;
}

int kz_version(void) { return KZ_VERSION; }

/* ------------------------------------------------------------ LZ77 */
typedef struct {
    const uint8_t *buf;
    int64_t n, start, last, ins;
    int32_t *head, *head3, *head2, *prev, *lhead, *head6, *prev6;
    int use6, use_bt, btd;
    int32_t *son;
    int64_t bmask, bt_next, pmask, plim;
    const uint8_t *rnd;
    int nice;
} ctx_t;

/* chain hash: 4 bytes (a 3-byte hash would mix far more unrelated positions into each chain) */
static inline uint32_t h4(const uint8_t *p) {
    uint32_t x;
    memcpy(&x, p, 4);
    return (x * 2654435761u) >> (32 - HBITS);
}
static inline uint32_t h3s(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> (32 - H3BITS);
}

/* content-defined anchor: ~1 in 256 positions, the same wherever a repeated blob sits */
static inline int anchorR(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> (32 - 4) == 0;
}
static inline int anchor3(const uint8_t *p) {
    return ((((uint32_t)p[0] << 16) | ((uint32_t)p[1] << 8) | p[2]) * 2654435761u) >> 24 == 0;
}

/* long-chain hash: 8 bytes, so every entry of that chain shares a long prefix with the position */
static inline uint32_t h6s(const uint8_t *p) {
    uint64_t x;
    memcpy(&x, p, 8);
    return (uint32_t)((x * 0x9E3779B97F4A7C15ull) >> (64 - H6BITS));
}


/* Binary-tree match finder (as in LZMA's bt4): one tree per 4-byte hash, every inserted
 * position is a node whose two sons are older positions that sort before/after it. Walking
 * the tree finds each longer match in turn (lengths and distances both ascending) and
 * inserts `pos` as the new root in the same pass. rec=0 only inserts. */
static int bt_run(ctx_t *c, int64_t pos, int depth, uint32_t *pl, uint32_t *pd, int np, int best, int rec) {
    const uint8_t *b = c->buf;
    int64_t rem = c->n - pos;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int32_t cur = c->head[h4(b + pos)];
    int32_t *son = c->son;
    int64_t mask = c->bmask;
    int32_t *ptr1 = son + ((pos & mask) << 1);
    int32_t *ptr0 = ptr1 + 1;
    int len0 = 0, len1 = 0, nice = c->nice;
    const uint8_t *cb = b + pos;
    for (;;) {
        if (cur < 0 || depth-- <= 0 || pos - cur > mask) { *ptr0 = *ptr1 = -1; break; }
        int32_t *pair = son + (((int64_t)cur & mask) << 1);
        const uint8_t *pb = b + cur;
        int len = len0 < len1 ? len0 : len1;
        if (pb[len] == cb[len]) {
            len++;
            while (len + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, pb + len, 8);
                memcpy(&y, cb + len, 8);
                if (x != y) { len += __builtin_ctzll(x ^ y) >> 3; goto ext_done; }
                len += 8;
            }
            while (len < limit && pb[len] == cb[len]) len++;
        ext_done:
            if (len > best) {
                best = len;
                if (rec) { pl[np] = (uint32_t)len; pd[np] = (uint32_t)(pos - cur); np++; }
            }
            if (len >= limit || len >= nice) { *ptr1 = pair[0]; *ptr0 = pair[1]; break; }
        }
        if (pb[len] < cb[len]) { *ptr1 = cur; ptr1 = pair + 1; cur = *ptr1; len1 = len; }
        else { *ptr0 = cur; ptr0 = pair; cur = *ptr0; len0 = len; }
    }
    return np;
}

static inline void put1(ctx_t *c, int64_t p) {
    if (c->use6 && p + 8 <= c->n) {
        uint32_t h6 = h6s(c->buf + p);
        c->prev6[p & R6MASK] = c->head6[h6];
        c->head6[h6] = (int32_t)p;
    }
    uint32_t h = h4(c->buf + p);
    if (c->use_bt && p >= c->bt_next && p + 4 <= c->n) {
        bt_run(c, p, p < c->start ? HISTD : c->btd, NULL, NULL, 0, 1 << 30, 0);
        c->bt_next = p + 1;
    }
    c->prev[p & c->pmask] = c->head[h];
    c->head[h] = (int32_t)p;
    c->head3[h3s(c->buf + p)] = (int32_t)p;
    c->head2[((unsigned)c->buf[p] << 8) | c->buf[p + 1]] = (int32_t)p;
}

static inline uint32_t h8(const uint8_t *p) {
    uint64_t x;
    memcpy(&x, p, 8);
    return (uint32_t)((x * 0x9E3779B97F4A7C15ull) >> (64 - LBITS));
}

/* long-match table: every 4th position in [a, b) (not in random chunks) */
static void lput(ctx_t *c, int64_t a, int64_t b) {
    int64_t lim = c->n - 8;
    a = (a + 3) & ~(int64_t)3;
    for (; a < b && a <= lim; a += 4) c->lhead[h8(c->buf + a)] = (int32_t)a;
}

/* hash every position below `upto` that hasn't been hashed yet */
static void insert_to(ctx_t *c, int64_t upto) {
    int64_t p = c->ins;
    int64_t stop = upto < c->last ? upto : c->last;
    while (p < stop) {
        int64_t ch = p >> RND_BITS;
        int64_t q = (ch + 1) << RND_BITS;
        if (q > stop) q = stop;
        uint8_t k = c->rnd[ch];
        if (k != 1 && k != 2) lput(c, p, q);
        if (k == 2) {
            /* literal-only chunk: hash only content-defined anchors */
            const uint8_t *b = c->buf;
            for (int64_t a = p; a < q; a++)
                if (anchor3(b + a)) put1(c, a);
        } else if (k == 1) {
            /* random chunk: only anchors (0x00 bytes) */
            const uint8_t *b = c->buf;
            for (int64_t a = p; a < q; a++)
                if (anchorR(b + a)) put1(c, a);
        } else if (p < c->start) {
            int64_t q2 = q < c->start ? q : c->start;
            int64_t nearp = c->start - HIST_NEAR;
            int64_t a = p;
            for (; a < q2 && a < nearp; a += HIST_FAR_STRIDE) put1(c, a);
            for (; a < q2; a += HIST_STRIDE) put1(c, a);
            for (int64_t a2 = q2; a2 < q; a2++) put1(c, a2);
        } else {
            for (int64_t a = p; a < q; a++) put1(c, a);
        }
        p = q;
    }
    if (upto > c->ins) c->ins = upto;
}

static void find_chain(ctx_t *c, int64_t i, int chain, int *ol, int *od) {
    *ol = 0;
    *od = 0;
    if (i >= c->last) return;
    const uint8_t *b = c->buf;
    int32_t j = c->head[h4(b + i)];
    int best_len = 0, best_dist = 0;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    while (j >= 0 && chain) {
        if (i - j > c->plim) break;
        if (best_len == 0 || b[j + best_len] == b[i + best_len]) {
            int l = 0;
            while (l + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, b + j + l, 8);
                memcpy(&y, b + i + l, 8);
                if (x != y) break;
                l += 8;
            }
            while (l < limit && b[j + l] == b[i + l]) l++;
            if (l > best_len) {
                best_len = l;
                best_dist = (int)(i - j);
                if (l >= limit || l >= c->nice) break;
            }
        }
        j = c->prev[j & c->pmask];
        chain--;
    }
    if (best_len < 3) {                      /* the nearest 3-byte match */
        int32_t j3 = c->head3[h3s(b + i)];
        if (j3 >= 0 && i - j3 <= WINDOW) {
            int l = 0;
            while (l < limit && b[j3 + l] == b[i + l]) l++;
            if (l >= 3) { best_len = l; best_dist = (int)(i - j3); }
        }
    }
    /* cost filter: a far short match costs more bits than literals */
    if (best_len < MIN_MATCH || (best_len == 3 && best_dist > 8192) ||
        (best_len == 4 && best_dist > 131072) ||
        (best_len <= 5 && best_dist > (1 << 20)) ||
        (best_len <= 7 && best_dist > (1 << 22)))
        return;
    if (best_len < SPARSE_MATCH && c->rnd[i >> RND_BITS] == 3) return;
    *ol = best_len;
    *od = best_dist;
}

/* chain search, then (when that found nothing long) one probe of the long-match table */
static void find(ctx_t *c, int64_t i, int chain, int *ol, int *od) {
    find_chain(c, i, chain, ol, od);
    if (*ol >= LONG_MIN || i + 8 > c->n) return;
    int32_t j = c->lhead[h8(c->buf + i)];
    if (j < 0 || j >= i || i - j > WINDOW) return;
    const uint8_t *b = c->buf;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int l = 0;
    while (l + 8 <= limit) {
        uint64_t x, y;
        memcpy(&x, b + j + l, 8);
        memcpy(&y, b + i + l, 8);
        if (x != y) break;
        l += 8;
    }
    while (l < limit && b[j + l] == b[i + l]) l++;
    if (l >= LONG_MIN && l > *ol) { *ol = l; *od = (int)(i - j); }
}

/* how far the match at distance d continues from pos */
static int run_len(const uint8_t *b, int64_t pos, int64_t n, int d) {
    int64_t rem = n - pos;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int l = 0;
    while (l + 8 <= limit) {
        uint64_t x, y;
        memcpy(&x, b + pos - d + l, 8);
        memcpy(&y, b + pos + l, 8);
        if (x != y) break;
        l += 8;
    }
    while (l < limit && b[pos - d + l] == b[pos + l]) l++;
    return l;
}

/* ------------------------------------------------------------ range coder */
/* Adaptive binary range coder (LZMA-style carry handling) over the LZ77 tokens.
 * Per block, models start fresh. Each probability is two 16-bit estimators (fast
 * and slow) averaged to 12 bits. Literals use the previous byte's top RC_LC bits
 * as context; right after a match they are coded against the byte the last
 * distance would have predicted ("matched literal"). A match whose distance is one
 * of the last four used (rep0..rep3) is coded as a rep; length 1 at rep0 is a
 * "short rep". Other lengths and distances use bit-trees (distance slot =
 * bit_length(d-1), top 3 extra bits modelled, the low 4 "align" bits modelled,
 * the rest direct).
 * Token rules: a literal is 0..255; a match is (len << 32) | dist with len >= 3,
 * except that a rep distance may have len 2 and rep0 may have len 1. */
#define RC_TOP (1u << 24)
#define RC_LC 3
#ifndef RC_PB
#define RC_PB 2          /* position bits (mod 2^RC_PB) in the match/literal state contexts */
#endif
#define RC_LPMAX 2       /* literal position bits: 0 or 2, chosen per block (stored in the first payload byte) */
#define RC_NS (8 << RC_PB)
#define PHS(x) ((unsigned)(x) & ((1u << RC_PB) - 1))
#define LCTX(m, prev, x) (((unsigned)(prev) >> (8 - RC_LC)) + (8u * ((unsigned)(x) & (m)->lpm)))
#ifndef RC_RA
#define RC_RA 4
#define RC_RB 7
#endif
#ifndef RC_K
#define RC_K 1.6
#endif
#define RC_NMAX 255
#ifndef RC_N0
#define RC_N0 0
#endif
typedef struct { uint16_t a, b; uint8_t n; } rcp_t;
typedef struct { rcp_t ch[2], low[8], mid[8], high[256]; } lenc_t;
typedef struct {
    rcp_t ism[RC_NS], isrep[RC_NS], isrep0[RC_NS], isrep1[RC_NS], isrep2[RC_NS], isrep0l[RC_NS];
    rcp_t lit[(1 << RC_LC) << RC_LPMAX][0x300];
    lenc_t ml, rl;
    rcp_t slot[4][32];
    rcp_t ext[27][8];
    rcp_t align[16];
    unsigned lpm;           /* literal position mask; keep last (rc_init skips it) */
    struct mix_s *mix;      /* literal mixer (NULL = off) */
} rcm_t;

/* Literal mixer: for a literal that does not follow a match, the bit probability is a
 * logistic mix of three models: the usual one (previous byte's top bits + position),
 * the full previous byte, and a hash of the previous two bytes. */
#define MIX_N 3
#define MIX_SHIFT 10
typedef struct mix_s { rcp_t t1[256][256]; rcp_t t2[4096][256]; int32_t w[256][MIX_N]; int32_t wm[256][MIX_N]; } mix_t;

/* count-based adaptive rate: a state moves by 1/(n+RC_K) until the rate reaches its floor
 * (2^-RC_RA fast, 2^-RC_RB slow), so a fresh context learns quickly and a seasoned one is steady */
static uint16_t RCR_A[256], RCR_B[256];
static int rcr_ready;
static void rcr_init(void) {
    for (int n = 0; n < 256; n++) {
        double r = 1.0 / (n + RC_K);
        double fa = 1.0 / (1 << RC_RA), fb = 1.0 / (1 << RC_RB);
        RCR_A[n] = (uint16_t)((r > fa ? r : fa) * 65536.0 + 0.5);
        RCR_B[n] = (uint16_t)((r > fb ? r : fb) * 65536.0 + 0.5);
    }
    rcr_ready = 1;
}
static inline uint32_t rc_p(const rcp_t *p) { return ((uint32_t)p->a + p->b) >> 5; }
static inline void rc_up(rcp_t *p, int bit) {
    unsigned n = p->n;
    if (n < RC_NMAX) p->n = (uint8_t)(n + 1);
    int ra = RCR_A[n], rb = RCR_B[n];
    if (bit) { p->a -= (uint16_t)(((uint32_t)p->a * ra) >> 16); p->b -= (uint16_t)(((uint32_t)p->b * rb) >> 16); }
    else { p->a += (uint16_t)(((uint32_t)(65535 - p->a) * ra) >> 16); p->b += (uint16_t)(((uint32_t)(65535 - p->b) * rb) >> 16); }
}
static void rc_init(rcm_t *m) {
    if (!rcr_ready) rcr_init();
    rcp_t *p = (rcp_t *)m;
    for (size_t i = 0; i < offsetof(rcm_t, lpm) / sizeof(rcp_t); i++) { p[i].a = 32768; p[i].b = 32768; p[i].n = RC_N0; }
    m->lpm = 0;
    m->mix = NULL;
}

static int16_t MIXSTR[4096];
static uint16_t MIXSQ[4096];
static int mix_ready;
static int mix_sq(int d) {
    static const int t[33] = {1,2,3,6,10,16,27,45,73,120,194,310,488,747,1101,1546,2047,2549,2994,3348,3607,3785,3901,3975,4022,4050,4068,4079,4085,4089,4092,4093,4094};
    if (d > 2047) return 4095;
    if (d < -2047) return 0;
    int w = d & 127;
    d = (d >> 7) + 16;
    return (t[d] * (128 - w) + t[d + 1] * w + 64) >> 7;
}
static void mix_tables(void) {
    int pi = 0;
    for (int x = -2047; x <= 2047; x++) {
        int v = mix_sq(x);
        for (int i = pi; i <= v; i++) MIXSTR[i] = (int16_t)x;
        pi = v + 1;
    }
    for (int i = pi; i < 4096; i++) MIXSTR[i] = 2047;
    for (int i = 0; i < 4096; i++) MIXSQ[i] = (uint16_t)mix_sq(i - 2048);
    mix_ready = 1;
}
static mix_t *mix_new(void) {
    if (!mix_ready) mix_tables();
    if (!rcr_ready) rcr_init();
    mix_t *x = (mix_t *)malloc(sizeof(mix_t));
    if (!x) return NULL;
    rcp_t *p = (rcp_t *)x;
    for (size_t i = 0; i < offsetof(mix_t, w) / sizeof(rcp_t); i++) { p[i].a = 32768; p[i].b = 32768; p[i].n = RC_N0; }
    for (int c = 0; c < 256; c++) { x->w[c][0] = 26000; x->w[c][1] = 20000; x->w[c][2] = 20000; x->wm[c][0] = 36000; x->wm[c][1] = 14000; x->wm[c][2] = 14000; }
    return x;
}
static inline unsigned mix_h2(unsigned prev2, unsigned prev) { return ((prev2 * 0x9E3779B1u) ^ (prev * 0x85EBCA6Bu)) >> 20; }
/* one bit: gathers the three inputs, returns P(bit = 0) in 1..4095; *ps receives what update needs */
static inline int mix_pred(mix_t *x, rcp_t *a, rcp_t *b, rcp_t *c, int32_t *w, int *s) {
    s[0] = MIXSTR[rc_p(a)]; s[1] = MIXSTR[rc_p(b)]; s[2] = MIXSTR[rc_p(c)];
    int64_t dot = (int64_t)s[0] * w[0] + (int64_t)s[1] * w[1] + (int64_t)s[2] * w[2];
    int d = (int)(dot >> 16);
    if (d > 2047) d = 2047; else if (d < -2047) d = -2047;
    int p = MIXSQ[d + 2048];
    return p < 1 ? 1 : p > 4095 ? 4095 : p;
}
static inline void mix_upd(rcp_t *a, rcp_t *b, rcp_t *c, int32_t *w, const int *s, int p, int bit) {
    int err = (bit ? 0 : 4095) - p;
    for (int k = 0; k < MIX_N; k++) w[k] += (s[k] * err) >> MIX_SHIFT;
}

typedef struct {
    uint64_t low;
    uint32_t range;
    uint8_t cache;
    int64_t csize, pos, cap;
    uint8_t *out;
    int err;
} renc_t;

static void rc_put(renc_t *e, uint8_t v) {
    if (e->pos < e->cap) e->out[e->pos++] = v; else e->err = 1;
}
static void rc_shift(renc_t *e) {
    if ((uint32_t)e->low < 0xFF000000u || (e->low >> 32)) {
        uint8_t carry = (uint8_t)(e->low >> 32), t = e->cache;
        do { rc_put(e, (uint8_t)(t + carry)); t = 0xFF; } while (--e->csize);
        e->cache = (uint8_t)(e->low >> 24);
    }
    e->csize++;
    e->low = (e->low & 0x00FFFFFFu) << 8;
}
static inline void rc_bit(renc_t *e, rcp_t *p, int bit) {
    uint32_t bound = (e->range >> 12) * rc_p(p);
    if (!bit) e->range = bound; else { e->low += bound; e->range -= bound; }
    rc_up(p, bit);
    while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
}
static inline void rc_bitp(renc_t *e, uint32_t p, int bit) {
    uint32_t bound = (e->range >> 12) * p;
    if (!bit) e->range = bound; else { e->low += bound; e->range -= bound; }
    while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
}
static inline void rc_mixlit(renc_t *e, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned byte) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned c0 = 1;
    for (int i = 7; i >= 0; i--) {
        int bit = (byte >> i) & 1, s[MIX_N];
        int32_t *w = x->w[c0];
        int p = mix_pred(x, &lp[c0], &t1[c0], &t2[c0], w, s);
        rc_bitp(e, (uint32_t)p, bit);
        mix_upd(&lp[c0], &t1[c0], &t2[c0], w, s, p, bit);
        rc_up(&lp[c0], bit); rc_up(&t1[c0], bit); rc_up(&t2[c0], bit);
        c0 = (c0 << 1) | (unsigned)bit;
    }
}
static inline void rc_mixlitm(renc_t *e, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned mb, unsigned byte) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned offs = 0x100, sym = 1;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1, s[MIX_N];
        int32_t *w = x->wm[sym];
        rcp_t *a = &lp[offs + mbit + sym];
        int p = mix_pred(x, a, &t1[sym], &t2[sym], w, s);
        rc_bitp(e, (uint32_t)p, bit);
        mix_upd(&lp[0], &t1[0], &t2[0], w, s, p, bit);
        rc_up(a, bit); rc_up(&t1[sym], bit); rc_up(&t2[sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
}
static inline void rc_direct(renc_t *e, uint32_t v, int n) {
    for (int i = n - 1; i >= 0; i--) {
        e->range >>= 1;
        if ((v >> i) & 1) e->low += e->range;
        while (e->range < RC_TOP) { e->range <<= 8; rc_shift(e); }
    }
}
static inline void rc_tree(renc_t *e, rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1;
    for (int i = nbits - 1; i >= 0; i--) {
        int b = (v >> i) & 1;
        rc_bit(e, &t[m], b);
        m = (m << 1) | b;
    }
}
static inline void rc_rtree(renc_t *e, rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1;
    for (int i = 0; i < nbits; i++) {
        int b = (v >> i) & 1;
        rc_bit(e, &t[m], b);
        m = (m << 1) | b;
    }
}
static inline void rc_len(renc_t *e, lenc_t *l, uint32_t v) {
    if (v < 8) { rc_bit(e, &l->ch[0], 0); rc_tree(e, l->low, 3, v); }
    else if (v < 16) { rc_bit(e, &l->ch[0], 1); rc_bit(e, &l->ch[1], 0); rc_tree(e, l->mid, 3, v - 8); }
    else { rc_bit(e, &l->ch[0], 1); rc_bit(e, &l->ch[1], 1); rc_tree(e, l->high, 8, v - 16); }
}
static inline void rc_dist(renc_t *e, rcm_t *m, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = 0;
    while (slot < 32 && (d >> slot)) slot++;
    rc_tree(e, m->slot[SLOTCTX(v)], 5, (uint32_t)slot);
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, r = eb - nm;
        rc_tree(e, m->ext[slot], nm, x >> r);
        if (r >= 4) {
            rc_direct(e, (x >> 4) & ((1u << (r - 4)) - 1), r - 4);
            rc_rtree(e, m->align, 4, x & 15);
        } else if (r > 0) {
            rc_direct(e, x & ((1u << r) - 1), r);
        }
    }
}
/* literal coded against mb, the byte a rep0 match would have produced */
static inline void rc_litm(renc_t *e, rcp_t *p, unsigned mb, unsigned byte) {
    unsigned offs = 0x100, sym = 1;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1;
        rc_bit(e, &p[offs + mbit + sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
}

static inline int rep_find(const uint32_t *rep, uint32_t len, uint32_t dist) {
    if (len < 2) return -1;
    for (int k = 0; k < 4; k++) if (rep[k] == dist) return k;
    return -1;
}

/* Encode one token (updating the models, the history state and *pos). Returns 1 if the
 * token breaks the rules (see above), else 0. */
static inline int enc_tok(renc_t *e, rcm_t *m, uint64_t t, const uint8_t *buf, int64_t pos0,
                          int64_t *ppos, unsigned *pst, uint32_t *rep) {
    int64_t pos = *ppos;
    unsigned st = *pst;
    unsigned sp = st + 8 * PHS(pos - pos0);
    if (t < 256) {
        unsigned prev = pos > pos0 ? buf[pos - 1] : 0;
        rc_bit(e, &m->ism[sp], 0);
        rcp_t *lp = m->lit[LCTX(m, prev, pos - pos0)];
        if (st & 1) { if (m->mix) rc_mixlitm(e, m->mix, lp, prev, pos - 1 > pos0 ? buf[pos - 2] : 0, buf[pos - rep[0]], t); else rc_litm(e, lp, buf[pos - rep[0]], t); }
        else if (m->mix) rc_mixlit(e, m->mix, lp, prev, pos - 1 > pos0 ? buf[pos - 2] : 0, t);
        else rc_tree(e, lp, 8, t);
        pos++;
        st = (st << 1) & 7;
    } else {
        uint32_t len = (uint32_t)(t >> DBITS), dist = (uint32_t)(t & DMASK);
        int ri = rep_find(rep, len, dist);
        if (len < 2 && ri < 0 && !(len == 1 && dist == rep[0])) return 1;
        rc_bit(e, &m->ism[sp], 1);
        if (ri >= 0 || len == 1) {
            rc_bit(e, &m->isrep[sp], 1);
            if (len == 1) {
                rc_bit(e, &m->isrep0[sp], 0);
                rc_bit(e, &m->isrep0l[sp], 0);
            } else {
                if (ri == 0) {
                    rc_bit(e, &m->isrep0[sp], 0);
                    rc_bit(e, &m->isrep0l[sp], 1);
                } else {
                    rc_bit(e, &m->isrep0[sp], 1);
                    if (ri == 1) rc_bit(e, &m->isrep1[sp], 0);
                    else { rc_bit(e, &m->isrep1[sp], 1); rc_bit(e, &m->isrep2[sp], ri == 3); }
                    for (int k = ri; k > 0; k--) rep[k] = rep[k - 1];
                    rep[0] = dist;
                }
                rc_len(e, &m->rl, len - 2);
            }
        } else {
            rc_bit(e, &m->isrep[sp], 0);
            uint32_t v = len - 2;
            rc_len(e, &m->ml, v);
            rc_dist(e, m, v, dist);
            rep[3] = rep[2]; rep[2] = rep[1]; rep[1] = rep[0]; rep[0] = dist;
        }
        pos += len;
        st = ((st << 1) | 1) & 7;
    }
    *ppos = pos;
    *pst = st;
    return 0;
}

/* Encode a token block. buf[pos0..] holds the bytes the tokens produce (only the byte
 * before each token is read, as literal context; before the first token it counts as 0).
 * Returns the payload size, or -1 if it doesn't fit in cap (or a token breaks the rules).
 * *raw_out = bytes the tokens expand to. */
int64_t kz_rc_enc(const uint64_t *tok, int64_t nt, const uint8_t *buf, int64_t pos0,
                  uint8_t *out, int64_t cap, int64_t *raw_out, int lp) {
    if (lp < 0 || (lp & 3) > RC_LPMAX || (lp >> 2) > 1) return -1;
    int64_t total = 0;
    for (int64_t i = 0; i < nt; i++) total += tok[i] < 256 ? 1 : (int64_t)(tok[i] >> DBITS);
    *raw_out = total;
    rcm_t *m = (rcm_t *)malloc(sizeof(rcm_t));
    if (!m) return -1;
    rc_init(m);
    m->lpm = (1u << (lp & 3)) - 1;
    if (lp >> 2) { m->mix = mix_new(); if (!m->mix) { free(m); return -1; } }
    renc_t e = {0, 0xFFFFFFFFu, 0, 1, 0, cap, out, 0};
    int64_t pos = pos0;
    unsigned st = 0;
    uint32_t rep[4] = {1, 1, 1, 1};
    for (int64_t i = 0; i < nt && !e.err; i++)
        if (enc_tok(&e, m, tok[i], buf, pos0, &pos, &st, rep)) { e.err = 1; break; }
    for (int k = 0; k < 5; k++) rc_shift(&e);
    free(m->mix);
    free(m);
    if (e.err || e.pos < 1) return -1;
    out[0] = (uint8_t)lp;      /* the coder's first byte is always 0; it carries the literal position bits */
    return e.pos;
}

typedef struct {
    const uint8_t *pl;
    int64_t plen, p;
    uint32_t range, code;
} rdec_t;

static inline uint8_t rd_byte(rdec_t *d) {
    uint8_t b = d->p < d->plen ? d->pl[d->p] : 0;
    d->p++;
    return b;
}
static inline int rd_bit(rdec_t *d, rcp_t *p) {
    uint32_t bound = (d->range >> 12) * rc_p(p);
    int bit;
    if (d->code < bound) { d->range = bound; bit = 0; }
    else { d->code -= bound; d->range -= bound; bit = 1; }
    rc_up(p, bit);
    while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    return bit;
}
static inline int rd_bitp(rdec_t *d, uint32_t p) {
    uint32_t bound = (d->range >> 12) * p;
    int bit;
    if (d->code < bound) { d->range = bound; bit = 0; }
    else { d->code -= bound; d->range -= bound; bit = 1; }
    while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    return bit;
}
static inline unsigned rd_mixlit(rdec_t *d, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned c0 = 1;
    for (int i = 0; i < 8; i++) {
        int s[MIX_N];
        int32_t *w = x->w[c0];
        int p = mix_pred(x, &lp[c0], &t1[c0], &t2[c0], w, s);
        int bit = rd_bitp(d, (uint32_t)p);
        mix_upd(&lp[c0], &t1[c0], &t2[c0], w, s, p, bit);
        rc_up(&lp[c0], bit); rc_up(&t1[c0], bit); rc_up(&t2[c0], bit);
        c0 = (c0 << 1) | (unsigned)bit;
    }
    return c0 & 0xFF;
}
static inline unsigned rd_mixlitm(rdec_t *d, mix_t *x, rcp_t *lp, unsigned prev, unsigned prev2, unsigned mb) {
    rcp_t *t1 = x->t1[prev], *t2 = x->t2[mix_h2(prev2, prev)];
    unsigned offs = 0x100, sym = 1;
    while (sym < 0x100) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int s[MIX_N];
        int32_t *w = x->wm[sym];
        rcp_t *a = &lp[offs + mbit + sym];
        int p = mix_pred(x, a, &t1[sym], &t2[sym], w, s);
        int bit = rd_bitp(d, (uint32_t)p);
        mix_upd(&lp[0], &t1[0], &t2[0], w, s, p, bit);
        rc_up(a, bit); rc_up(&t1[sym], bit); rc_up(&t2[sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
    return sym & 0xFF;
}
static inline uint32_t rd_direct(rdec_t *d, int n) {
    uint32_t v = 0;
    for (int i = 0; i < n; i++) {
        d->range >>= 1;
        int b = d->code >= d->range;
        if (b) d->code -= d->range;
        v = (v << 1) | (uint32_t)b;
        while (d->range < RC_TOP) { d->range <<= 8; d->code = (d->code << 8) | rd_byte(d); }
    }
    return v;
}
static inline uint32_t rd_tree(rdec_t *d, rcp_t *t, int nbits) {
    uint32_t m = 1;
    for (int i = 0; i < nbits; i++) m = (m << 1) | (uint32_t)rd_bit(d, &t[m]);
    return m - (1u << nbits);
}
static inline uint32_t rd_rtree(rdec_t *d, rcp_t *t, int nbits) {
    uint32_t m = 1, v = 0;
    for (int i = 0; i < nbits; i++) {
        int b = rd_bit(d, &t[m]);
        m = (m << 1) | (uint32_t)b;
        v |= (uint32_t)b << i;
    }
    return v;
}
static inline uint32_t rd_len(rdec_t *d, lenc_t *l) {
    if (!rd_bit(d, &l->ch[0])) return rd_tree(d, l->low, 3);
    if (!rd_bit(d, &l->ch[1])) return 8 + rd_tree(d, l->mid, 3);
    return 16 + rd_tree(d, l->high, 8);
}

/* Decode a range-coded block, appending raw bytes at out[op..op+raw). Same error codes as kz_decode. */
int kz_rc_dec(const uint8_t *pl, int64_t plen, uint8_t *out, int64_t outcap, int64_t op, int64_t raw) {
    int64_t target = op + raw, op0 = op;
    if (raw < 0 || target > outcap) return -4;
    if (plen < 5 || (pl[0] & 3) > RC_LPMAX || (pl[0] >> 2) > 1) return -1;
    rcm_t *m = (rcm_t *)malloc(sizeof(rcm_t));
    if (!m) return -7;
    rc_init(m);
    m->lpm = (1u << (pl[0] & 3)) - 1;
    if (pl[0] >> 2) { m->mix = mix_new(); if (!m->mix) { free(m); return -7; } }
    rdec_t d = {pl, plen, 1, 0xFFFFFFFFu, 0};
    for (int k = 0; k < 4; k++) d.code = (d.code << 8) | rd_byte(&d);
    unsigned st = 0;
    uint32_t rep[4] = {1, 1, 1, 1};
    int rc = 0;
    while (op < target) {
        if (d.p > plen + 8) { rc = -2; break; }
        unsigned sp = st + 8 * PHS(op - op0);
        if (!rd_bit(&d, &m->ism[sp])) {
            unsigned prev = op > op0 ? out[op - 1] : 0;
            rcp_t *lp = m->lit[LCTX(m, prev, op - op0)];
            if ((st & 1) && m->mix) {
                out[op] = (uint8_t)rd_mixlitm(&d, m->mix, lp, prev, op - 1 > op0 ? out[op - 2] : 0, out[op - rep[0]]);
                op++;
            } else             if (st & 1) {
                unsigned mb = out[op - rep[0]], offs = 0x100, sym = 1;
                while (sym < 0x100) {
                    mb <<= 1;
                    unsigned mbit = mb & offs;
                    int bit = rd_bit(&d, &lp[offs + mbit + sym]);
                    sym = (sym << 1) | (unsigned)bit;
                    if (bit) offs &= mbit; else offs &= ~mbit;
                }
                out[op++] = (uint8_t)sym;
            } else {
                if (m->mix) {
                    unsigned v = rd_mixlit(&d, m->mix, lp, prev, op - 1 > op0 ? out[op - 2] : 0);
                    out[op++] = (uint8_t)v;
                }
                else out[op++] = (uint8_t)rd_tree(&d, lp, 8);
            }
            st = (st << 1) & 7;
            continue;
        }
        int64_t len;
        uint32_t dist;
        if (rd_bit(&d, &m->isrep[sp])) {
            if (!rd_bit(&d, &m->isrep0[sp])) {
                dist = rep[0];
                if (!rd_bit(&d, &m->isrep0l[sp])) len = 1;
                else len = (int64_t)rd_len(&d, &m->rl) + 2;
            } else {
                int ri;
                if (!rd_bit(&d, &m->isrep1[sp])) ri = 1;
                else ri = 2 + rd_bit(&d, &m->isrep2[sp]);
                dist = rep[ri];
                for (int k = ri; k > 0; k--) rep[k] = rep[k - 1];
                rep[0] = dist;
                len = (int64_t)rd_len(&d, &m->rl) + 2;
            }
        } else {
            uint32_t v = rd_len(&d, &m->ml);
            len = (int64_t)v + 2;
            uint32_t slot = rd_tree(&d, m->slot[SLOTCTX(v)], 5);
            if (slot > 26) { rc = -3; break; }
            uint32_t dd = slot;                       /* slots 0 and 1: d = slot */
            if (slot >= 2) {
                int eb = (int)slot - 1, nm = eb < 3 ? eb : 3, r = eb - nm;
                uint32_t x = rd_tree(&d, m->ext[slot], nm) << r;
                if (r >= 4) {
                    x |= rd_direct(&d, r - 4) << 4;
                    x |= rd_rtree(&d, m->align, 4);
                } else if (r > 0) {
                    x |= rd_direct(&d, r);
                }
                dd = (1u << eb) + x;
            }
            dist = dd + 1;
            rep[3] = rep[2]; rep[2] = rep[1]; rep[1] = rep[0]; rep[0] = dist;
        }
        if ((int64_t)dist > op) { rc = -3; break; }
        if (op + len > target) { rc = -4; break; }
        const uint8_t *src = out + op - dist;
        uint8_t *dst = out + op;
        if ((int64_t)dist >= len) memcpy(dst, src, (size_t)len);
        else for (int64_t k = 0; k < len; k++) dst[k] = src[k];
        op += len;
        st = ((st << 1) | 1) & 7;
    }
    free(m->mix);
    free(m);
    if (rc) return rc;
    if (d.p > plen) return -5;
    return 0;
}

/* ------------------------------------------------------------ parser state & prices */
/* At the optimal-parsing levels (lazy == 2) the parser keeps a live copy of the range
 * coder's models, updated as tokens are committed, and prices every candidate with it
 * (cost in 1/64 bit). It also tracks the coder's rep distances and history state, so a
 * token with len < 3 (only valid as a rep) is never emitted when the coder couldn't
 * code it: the coder resets at each block (every BLK tokens of a part). */
#define BLK 524288            /* tokens per range-coded block (BLOCK_TOKENS in kiln.py) */
#define OPTW 2048             /* positions parsed per optimal-parse window */
#define PINF 0x3FFFFFFF
static double log2_(double x);

static uint16_t PRC[4097];
static int prc_ready;
static void prc_init(void) {
    for (int p = 1; p <= 4096; p++) PRC[p] = (uint16_t)(-log2_(p / 4096.0) * 64.0 + 0.5);
    PRC[0] = PRC[1];
    prc_ready = 1;
}
static inline int pb(const rcp_t *p, int bit) {
    uint32_t q = rc_p(p);
    return PRC[bit ? 4096 - q : q];
}
static inline int ptree(const rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1; int s = 0;
    for (int i = nbits - 1; i >= 0; i--) { int b = (v >> i) & 1; s += pb(&t[m], b); m = (m << 1) | (uint32_t)b; }
    return s;
}
static inline int prtree(const rcp_t *t, int nbits, uint32_t v) {
    uint32_t m = 1; int s = 0;
    for (int i = 0; i < nbits; i++) { int b = (v >> i) & 1; s += pb(&t[m], b); m = (m << 1) | (uint32_t)b; }
    return s;
}
static int plen(const lenc_t *l, uint32_t v) {
    if (v < 8) return pb(&l->ch[0], 0) + ptree(l->low, 3, v);
    if (v < 16) return pb(&l->ch[0], 1) + pb(&l->ch[1], 0) + ptree(l->mid, 3, v - 8);
    return pb(&l->ch[0], 1) + pb(&l->ch[1], 1) + ptree(l->high, 8, v - 16);
}
static int pdist(const rcm_t *m, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = 0;
    if (d) slot = 32 - __builtin_clz(d);
    int s = ptree(m->slot[SLOTCTX(v)], 5, (uint32_t)slot);
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, r = eb - nm;
        s += ptree(m->ext[slot], nm, x >> r);
        if (r >= 4) s += (r - 4) * 64 + prtree(m->align, 4, x & 15);
        else s += r * 64;
    }
    return s;
}
static inline int plit(const rcm_t *m, unsigned lctx, unsigned byte, int matched, unsigned mb) {
    const rcp_t *p = m->lit[lctx];
    if (!matched) return ptree(p, 8, byte);
    unsigned offs = 0x100, sym = 1; int s = 0;
    for (int i = 7; i >= 0; i--) {
        mb <<= 1;
        unsigned mbit = mb & offs;
        int bit = (byte >> i) & 1;
        s += pb(&p[offs + mbit + sym], bit);
        sym = (sym << 1) | (unsigned)bit;
        if (bit) offs &= mbit; else offs &= ~mbit;
    }
    return s;
}

typedef struct {
    int32_t price, from;
    uint32_t len, dist;
    uint32_t rep[4];
    uint8_t st;
} onode_t;

typedef struct {
    const uint8_t *buf;
    uint64_t *tok;
    int64_t nt, pstart;
    int learn;
    rcm_t *m;
    renc_t e;
    uint8_t scr[512];
    uint32_t rep[4];
    unsigned st;
    int64_t pos, bpos0, lastref;
    int mlp[257], rlp[257];
    onode_t *nd;
    uint32_t *stk;
    int16_t dsl[4][32], dex[27][8], dal[16];   /* distance prices, fixed for one window */
} lzs_t;

/* The models only change between windows (tokens are emitted after a window is parsed), so the
 * distance prices are tabulated once per window instead of walking the bit trees per candidate. */
static void dtab(lzs_t *s) {
    const rcm_t *m = s->m;
    for (int c = 0; c < 4; c++) for (int sl = 0; sl < 32; sl++) s->dsl[c][sl] = (int16_t)ptree(m->slot[c], 5, (uint32_t)sl);
    for (int sl = 2; sl < 27; sl++) {
        int eb = sl - 1, nm = eb < 3 ? eb : 3;
        for (int x = 0; x < (1 << nm); x++) s->dex[sl][x] = (int16_t)ptree(m->ext[sl], nm, (uint32_t)x);
    }
    for (int a = 0; a < 16; a++) s->dal[a] = (int16_t)prtree(m->align, 4, (uint32_t)a);
}
static inline int pdist2(const lzs_t *s, uint32_t v, uint32_t dist) {
    uint32_t d = dist - 1;
    int slot = d ? 32 - __builtin_clz(d) : 0;
    int r = s->dsl[SLOTCTX(v)][slot];
    if (slot >= 2) {
        int eb = slot - 1;
        uint32_t x = d - (1u << eb);
        int nm = eb < 3 ? eb : 3, rr = eb - nm;
        r += s->dex[slot][x >> rr];
        r += rr >= 4 ? (rr - 4) * 64 + s->dal[x & 15] : rr * 64;
    }
    return r;
}

static void lzs_reset_block(lzs_t *s) {
    s->rep[0] = s->rep[1] = s->rep[2] = s->rep[3] = 1;
    s->st = 0;
    s->bpos0 = s->pos;
}

/* append a token; at the optimal levels also feed it to the live models */
static void emit(lzs_t *s, uint64_t t) {
    if (!s->learn) { s->tok[s->nt++] = t; return; }
    if ((s->nt - s->pstart) % BLK == 0) lzs_reset_block(s);
    if (t >= 256) {
        uint32_t len = (uint32_t)(t >> DBITS), dist = (uint32_t)(t & DMASK);
        if (len < 2 && rep_find(s->rep, len, dist) < 0 && !(len == 1 && dist == s->rep[0])) {
            for (uint32_t k = 0; k < len; k++) emit(s, s->buf[s->pos]);   /* not a valid rep here */
            return;
        }
    }
    s->tok[s->nt++] = t;
    s->e.pos = 0; s->e.err = 0; s->e.csize = 1;
    enc_tok(&s->e, s->m, t, s->buf, s->bpos0, &s->pos, &s->st, s->rep);
}

static void refresh_prices(lzs_t *s) {
    for (uint32_t v = 0; v < 257; v++) s->mlp[v] = plen(&s->m->ml, v);
    for (uint32_t v = 0; v < 257; v++) s->rlp[v] = plen(&s->m->rl, v);
    s->lastref = s->nt;
}

/* all matches at i that are longer than every nearer one: lens ascending, dists ascending */
static int find_all(ctx_t *c, int64_t i, int chain, uint32_t *pl, uint32_t *pd) {
    if (i >= c->last) return 0;
    const uint8_t *b = c->buf;
    int64_t rem = c->n - i;
    int limit = rem > MAX_MATCH ? MAX_MATCH : (int)rem;
    int best = 2, np = 0;
    {
        int32_t j3 = c->head3[h3s(b + i)];
        if (j3 >= 0 && i - j3 <= WINDOW) {
            int l = 0;
            while (l < limit && b[j3 + l] == b[i + l]) l++;
            if (l >= 3) { best = l; pl[0] = (uint32_t)l; pd[0] = (uint32_t)(i - j3); np = 1;
                          if (l >= limit || l >= c->nice) return np; }
        }
    }
#define TRY(J) do {                                                         \
        int32_t jj_ = (J);                                                  \
        if (b[jj_ + best] == b[i + best]) {                                 \
            int l = 0;                                                      \
            while (l + 8 <= limit) {                                        \
                uint64_t x, y;                                              \
                memcpy(&x, b + jj_ + l, 8);                                 \
                memcpy(&y, b + i + l, 8);                                   \
                if (x != y) break;                                          \
                l += 8;                                                     \
            }                                                               \
            while (l < limit && b[jj_ + l] == b[i + l]) l++;                \
            if (l > best) {                                                 \
                best = l;                                                   \
                pl[np] = (uint32_t)l; pd[np] = (uint32_t)(i - jj_); np++;   \
                if (l >= limit || l >= c->nice) return np;                  \
            }                                                               \
        }                                                                   \
    } while (0)
    if (c->use_bt && c->bt_next == i && i + 4 <= c->n && BTMODE == 2) {
        np = bt_run(c, i, c->btd, pl, pd, np, best, 1);
        c->bt_next = i + 1;
        if (np) best = (int)pl[np - 1];
        if (np && best >= c->nice) return np;
        goto long_tbl;
    }
    int32_t j = c->head[h4(b + i)];
    int32_t jlast = 0x7FFFFFFF;
    int s4 = chain / 4;
    if (s4 < 2) s4 = 2;
    while (j >= 0 && s4) {
        if (i - j > c->plim) { j = -1; break; }
        int32_t jn = c->prev[j & c->pmask];
        if (jn >= 0) __builtin_prefetch(b + jn, 0, 1);
        TRY(j);
        jlast = j;
        j = jn;
        s4--;
    }
    if (j >= 0 && c->use6 && i + 8 <= c->n) {
        /* the nearest occurrences of these 4 bytes are done: go further back along the long chain */
        int32_t k = c->head6[h6s(b + i)];
        int skip = 64;
        while (k >= 0 && k >= jlast && skip--) {
            if (i - k > R6MASK) { k = -1; break; }
            int32_t kn = c->prev6[k & R6MASK];
            if (kn >= k) { k = -1; break; }
            k = kn;
        }
        int s6 = chain / 3;
        while (k >= 0 && s6 && k < jlast) {
            if (i - k > WINDOW || i - k > R6MASK) break;
            int32_t kn = c->prev6[k & R6MASK];
            if (kn >= k) kn = -1;
            if (kn >= 0) __builtin_prefetch(b + kn, 0, 1);
            TRY(k);
            k = kn;
            s6--;
        }
    }
long_tbl:
    if (best < LONG_MIN && i + 8 <= c->n) {
        int32_t k = c->lhead[h8(b + i)];
        if (k >= 0 && k < i && i - k <= WINDOW) {
            int l = 0;
            while (l + 8 <= limit) {
                uint64_t x, y;
                memcpy(&x, b + k + l, 8);
                memcpy(&y, b + i + l, 8);
                if (x != y) break;
                l += 8;
            }
            while (l < limit && b[k + l] == b[i + l]) l++;
            if (l >= LONG_MIN && l > best) { pl[np] = (uint32_t)l; pd[np] = (uint32_t)(i - k); np++; }
        }
    }
    return np;
}

static inline void relax(onode_t *nd, int *lenEnd, int cur, int t, int price, uint32_t len,
                         uint32_t dist, uint32_t r0, uint32_t r1, uint32_t r2, uint32_t r3, unsigned st) {
    while (*lenEnd < t) nd[++*lenEnd].price = PINF;
    onode_t *q = &nd[t];
    if (price < q->price) {
        q->price = price; q->from = cur; q->len = len; q->dist = dist;
        q->rep[0] = r0; q->rep[1] = r1; q->rep[2] = r2; q->rep[3] = r3; q->st = (uint8_t)st;
    }
}

/* price of the "rep k" selector bits (after is_match=1 and is_rep=1), k = 0..3 */
static inline int psel(const rcm_t *m, unsigned sp, int k) {
    if (k == 0) return pb(&m->isrep0[sp], 0) + pb(&m->isrep0l[sp], 1);
    if (k == 1) return pb(&m->isrep0[sp], 1) + pb(&m->isrep1[sp], 0);
    return pb(&m->isrep0[sp], 1) + pb(&m->isrep1[sp], 1) + pb(&m->isrep2[sp], k == 3);
}

/* Parse a window starting at i0 for the cheapest token path, emit it, return the new position. */
static int64_t opt_window(lzs_t *s, ctx_t *c, int64_t i0, int chain, int nice, int64_t wend) {
    const uint8_t *b = c->buf;
    int64_t n = c->n;
    const rcm_t *m = s->m;
    onode_t *nd = s->nd;
    if (s->nt - s->lastref >= 256 || s->lastref == 0) refresh_prices(s);
    dtab(s);
    int lim = (int)(wend - i0 < OPTW ? wend - i0 : OPTW);
    if (lim < 1) lim = 1;
    nd[0].price = 0; nd[0].from = 0;
    for (int k = 0; k < 4; k++) nd[0].rep[k] = s->rep[k];
    nd[0].st = (uint8_t)s->st;
    int cur = 0, lenEnd = 0, fin = -1;
    uint32_t pl[MAX_MATCH], pd[MAX_MATCH];
    for (;;) {
        if (cur > 0 && cur >= lenEnd) { fin = cur; break; }
        if (cur >= lim) { fin = cur; break; }
        int64_t pos = i0 + cur;
        onode_t *N = &nd[cur];
        int P = N->price;
        unsigned st = N->st;
        unsigned sp = st + 8 * PHS(pos - s->bpos0);
        uint32_t R0 = N->rep[0], R1 = N->rep[1], R2 = N->rep[2], R3 = N->rep[3];
        int stop = 0;
        if (pos + 24 < n) {
            /* the tables are far bigger than the cache: start the loads a few positions early */
            __builtin_prefetch(&c->head[h4(b + pos + 12)], 1, 1);
            __builtin_prefetch(&c->lhead[h8(b + pos + 12)], 0, 1);
            if (c->use6) __builtin_prefetch(&c->head6[h6s(b + pos + 12)], 1, 1);
            int32_t jh = c->head[h4(b + pos + 5)];
            if (jh >= 0) { __builtin_prefetch(&c->prev[jh & c->pmask], 0, 1); __builtin_prefetch(b + jh, 0, 1); }
        }
        insert_to(c, pos);
        /* literal */
        {
            unsigned prev = pos > 0 ? b[pos - 1] : 0;
            int matched = (st & 1) && R0 <= (uint32_t)pos;
            int cost = pb(&m->ism[sp], 0) +
                       plit(m, LCTX(m, prev, pos - s->bpos0), b[pos], matched, matched ? b[pos - R0] : 0);
            relax(nd, &lenEnd, cur, cur + 1, P + cost, 1, 0, R0, R1, R2, R3, (st << 1) & 7);
        }
        int base1 = pb(&m->ism[sp], 1);
        int nst = ((st << 1) | 1) & 7;
        /* reps */
        {
            uint32_t rr[4] = {R0, R1, R2, R3};
            int isrep1 = base1 + pb(&m->isrep[sp], 1);
            for (int k = 0; k < 4 && !stop; k++) {
                uint32_t d = rr[k];
                int dup = 0;
                for (int j = 0; j < k; j++) if (rr[j] == d) dup = 1;
                if (dup || d > (uint64_t)pos) continue;
                int l = run_len(b, pos, n, (int)d);
                if (k == 0 && l >= 1)
                    relax(nd, &lenEnd, cur, cur + 1,
                          P + isrep1 + pb(&m->isrep0[sp], 0) + pb(&m->isrep0l[sp], 0), 1, d,
                          R0, R1, R2, R3, nst);
                if (l < 2) continue;
                int sel = P + isrep1 + psel(m, sp, k);
                uint32_t a0 = d, a1 = k == 0 ? R1 : R0, a2 = k <= 1 ? R2 : R1, a3 = k <= 2 ? R3 : R2;
                if (k == 0) { a1 = R1; a2 = R2; a3 = R3; }
                else if (k == 1) { a1 = R0; a2 = R2; a3 = R3; }
                else if (k == 2) { a1 = R0; a2 = R1; a3 = R3; }
                else { a1 = R0; a2 = R1; a3 = R2; }
                for (int L = 2; L <= l; L++)
                    relax(nd, &lenEnd, cur, cur + L, sel + s->rlp[L - 2], (uint32_t)L, d, a0, a1, a2, a3, nst);
                if (l >= nice) { fin = cur + l; stop = 1; }
            }
        }
        if (stop) break;
        /* new matches */
        if (pos < c->last) {
            int np = find_all(c, pos, chain, pl, pd);
            int basem = base1 + pb(&m->isrep[sp], 0);
            uint32_t prevL = 2;
            for (int p = 0; p < np; p++) {
                uint32_t d = pd[p], Lp = pl[p];
                if (d == R0 || d == R1 || d == R2 || d == R3) { prevL = Lp; continue; }  /* a rep: done above */
                int dc[4] = {-1, -1, -1, -1};
                for (uint32_t L = prevL + 1; L <= Lp; L++) {
                    if (L < 3) continue;
                    uint32_t v = L - 2;
                    int ctx = (int)SLOTCTX(v);
                    if (dc[ctx] < 0) dc[ctx] = pdist2(s, v, d);
                    relax(nd, &lenEnd, cur, cur + (int)L, P + basem + s->mlp[v] + dc[ctx], L, d,
                          d, R0, R1, R2, nst);
                }
                prevL = Lp;
            }
            /* a length-2 match at the nearest earlier occurrence of these 2 bytes */
            {
                int32_t j2 = c->head2[((unsigned)b[pos] << 8) | b[pos + 1]];
                if (j2 >= 0 && j2 < pos && pos - j2 <= WINDOW) {
                    uint32_t d2 = (uint32_t)(pos - j2);
                    if (!(d2 == R0 || d2 == R1 || d2 == R2 || d2 == R3))
                        relax(nd, &lenEnd, cur, cur + 2, P + basem + s->mlp[0] + pdist2(s, 0, d2), 2, d2,
                              d2, R0, R1, R2, nst);
                }
            }
            if (np && pl[np - 1] >= (uint32_t)nice) { fin = cur + (int)pl[np - 1]; break; }
        }
        cur++;
    }
    /* trace back and emit */
    int ne = 0;
    for (int x = fin; x > 0; x = nd[x].from) { s->stk[2 * ne] = nd[x].len; s->stk[2 * ne + 1] = nd[x].dist; ne++; }
    int64_t p = i0;
    for (int k = ne - 1; k >= 0; k--) {
        uint32_t len = s->stk[2 * k], dist = s->stk[2 * k + 1];
        if (dist == 0) emit(s, b[p]);
        else {
            emit(s, ((uint64_t)len << DBITS) | (uint64_t)dist);
            if (len >= SKIP_INSERT && p + len > c->ins) { insert_to(c, p + 4); c->ins = p + len; }
        }
        p += len;
    }
    return p;
}

/* Returns the number of tokens written to tok (a literal byte, or
 * (length << 23) | distance), or -1 on failure. A raw run (bytes stored
 * uncompressed) is reported as a pair in runs[]: (token index, byte count).
 * lazy: 0 greedy, 1 lazy matching, 2 optimal parsing. At lazy == 2 a match may also be
 * a rep (length 2 or 3.. at one of the last 4 distances, or length 1 at the last one). */
int64_t kz_lz(const uint8_t *buf, int64_t n, int64_t start, const uint8_t *rnd,
              int64_t nch, int chain_max, int nice, int lazy, int accel,
              uint64_t *tok, int64_t *runs, int64_t runcap, int64_t *nruns_out, int lp) {
    ctx_t c;
    lzs_t S;
    memset(&S, 0, sizeof S);
    c.buf = buf; c.n = n; c.start = start; c.last = n - 3; c.ins = 0;
    c.rnd = rnd; c.nice = nice;
    c.lhead = (int32_t *)malloc(sizeof(int32_t) << LBITS);
    c.head = (int32_t *)malloc(sizeof(int32_t) * ((size_t)1 << HBITS) + sizeof(int32_t) * (((size_t)1 << H3BITS) + 65536));
    c.head3 = c.head + ((size_t)1 << HBITS);
    c.head2 = c.head3 + ((size_t)1 << H3BITS);
    /* the hash-chain links are a ring, not one slot per byte: chain levels reach back 2^PRB_CHAIN (32 MB);
     * the optimal-parse levels only use it for short fallback chains, so 2^PRB_OPT is plenty */
    { int cap = lazy >= 2 ? PRB_OPT : PRB_CHAIN, bb = 10; while (bb < cap && ((int64_t)1 << bb) < n + 32) bb++;
      c.pmask = ((int64_t)1 << bb) - 1;
      c.plim = (((int64_t)1 << bb) >= n + 32) ? (int64_t)WINDOW : (c.pmask - 16 < (int64_t)WINDOW ? c.pmask - 16 : (int64_t)WINDOW); }
    c.prev = (int32_t *)malloc(sizeof(int32_t) * (size_t)(c.pmask + 1));
    c.use_bt = lazy >= 2; c.use6 = lazy >= 2 && !(BTMODE == 2);
    c.btd = chain_max * BTK / 4; c.bt_next = 0;
    { int bb = 1; while (bb < 24 && ((int64_t)1 << bb) < n) bb++; c.bmask = ((int64_t)1 << bb) - 1; }
    c.son = c.use_bt ? (int32_t *)malloc(sizeof(int32_t) * 2 * (size_t)(c.bmask + 1)) : (int32_t *)malloc(4);
    c.head6 = c.use6 ? (int32_t *)malloc(sizeof(int32_t) << H6BITS) : (int32_t *)malloc(4);
    c.prev6 = c.use6 ? (int32_t *)malloc(sizeof(int32_t) * (size_t)(n < (1 << R6BITS) ? (n > 0 ? n : 1) : (1 << R6BITS))) : (int32_t *)malloc(4);
    S.buf = buf; S.tok = tok; S.learn = lazy >= 2;
    if (S.learn) {
        if (!prc_ready) prc_init();
        S.m = (rcm_t *)malloc(sizeof(rcm_t));
        S.nd = (onode_t *)malloc(sizeof(onode_t) * (OPTW + MAX_MATCH + 8));
        S.stk = (uint32_t *)malloc(sizeof(uint32_t) * 2 * (OPTW + MAX_MATCH + 8));
        S.e.low = 0; S.e.range = 0xFFFFFFFFu; S.e.cache = 0; S.e.csize = 1;
        S.e.cap = sizeof S.scr; S.e.out = S.scr;
        if (S.m) { rc_init(S.m); S.m->lpm = (1u << (lp < 0 || lp > RC_LPMAX ? 0 : lp)) - 1; }
        lzs_reset_block(&S);
    }
#define LZ_FREE() do { free(c.head); free(c.prev); free(c.head6); free(c.prev6); free(c.son); free(c.lhead); free(S.m); free(S.nd); free(S.stk); } while (0)
    if (!c.head6 || !c.prev6 || !c.son || !c.head || !c.prev || !c.lhead || (S.learn && (!S.m || !S.nd || !S.stk))) { LZ_FREE(); return -1; }
    memset(c.head, 0xFF, sizeof(int32_t) * ((size_t)1 << HBITS) + sizeof(int32_t) * (((size_t)1 << H3BITS) + 65536));
    memset(c.lhead, 0xFF, sizeof(int32_t) << LBITS);
    if (c.use6) memset(c.head6, 0xFF, sizeof(int32_t) << H6BITS);

    int64_t nr = 0, i = start;
    int miss = 0, have_cache = 0, cl = 0, cd = 0;
    int64_t cpos = 0;
    insert_to(&c, start);

    while (i < n) {
        S.pos = i;
        uint8_t k = rnd[i >> RND_BITS];
        if (k == 2) {                        /* literal-only: straight to the entropy coder */
            int64_t j = (i >> RND_BITS) + 1;
            while (j < nch && rnd[j] == 2) j++;
            int64_t e = j << RND_BITS;
            if (e > n) e = n;
            int64_t a = i;
            int found = 0, l = 0, d = 0;
            for (;;) {                       /* probe only at anchors */
                while (a < e && a < c.last && !anchor3(buf + a)) a++;
                if (a >= e || a >= c.last) { a = -1; break; }
                insert_to(&c, a);
                find(&c, a, chain_max, &l, &d);
                if (l >= MIN_RND_MATCH) { found = 1; break; }
                a++;
            }
            int64_t end = found ? a : e;
            while (i < end) { emit(&S, buf[i]); i++; }
            if (found) {
                emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
                i = a + l;
                while (i < n) {              /* a repeated blob goes on: keep following it */
                    int l2 = run_len(buf, i, n, d);
                    if (l2 < 8) break;
                    emit(&S, ((uint64_t)l2 << DBITS) | (uint64_t)(uint32_t)d);
                    i += l2;
                }
            }
            have_cache = 0; miss = 0;
            continue;
        }
        if (k == 1) {                        /* random-looking: raw, probe anchors */
            int64_t j = (i >> RND_BITS) + 1;
            while (j < nch && rnd[j] == 1) j++;
            int64_t e = j << RND_BITS;
            if (e > n) e = n;
            int64_t a = i;
            int found = 0, l = 0, d = 0;
            for (;;) {
                if (a >= e) { a = -1; break; }
                while (a < e && a < c.last && !anchorR(buf + a)) a++;
                if (a >= e || a >= c.last) { a = -1; break; }
                insert_to(&c, a);
                find(&c, a, chain_max, &l, &d);
                if (l >= MIN_RND_MATCH) { found = 1; break; }
                a++;
            }
            int64_t end = found ? a : e;
            if (end > i) {
                if (nr >= runcap) { LZ_FREE(); return -1; }
                runs[2 * nr] = S.nt;
                runs[2 * nr + 1] = end - i;
                nr++;
                S.pstart = S.nt;
            }
            S.pos = end;
            if (found) {
                emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
                end += l;
                while (end < n) {            /* a repeated blob goes on: keep following it */
                    int l2 = run_len(buf, end, n, d);
                    if (l2 < 8) break;
                    emit(&S, ((uint64_t)l2 << DBITS) | (uint64_t)(uint32_t)d);
                    end += l2;
                }
            }
            i = end;
            have_cache = 0; miss = 0;
            continue;
        }
        int dry = (miss >= 32 && accel < 99);
        if (S.learn && k == 0 && !dry) {
            int64_t wend = ((i >> RND_BITS) + 1) << RND_BITS;
            if (wend > n) wend = n;
            int64_t t0 = S.nt;
            i = opt_window(&S, &c, i, chain_max, nice, wend);
            for (int64_t q = t0; q < S.nt; q++) { if (tok[q] < 256) miss++; else miss = 0; }
            have_cache = 0;
            continue;
        }
        insert_to(&c, i);
        int l, d;
        if (have_cache && cpos == i) {
            l = cl; d = cd;
        } else {
            find(&c, i, (dry || k == 3) ? DRY_CHAIN : chain_max, &l, &d);
        }
        have_cache = 0;
        if (l) {
            miss = 0;
            if (lazy && l < LAZY_SKIP && i + 1 < n) {
                insert_to(&c, i + 1);
                int l2, d2;
                find(&c, i + 1, chain_max, &l2, &d2);
                if (l2 > l) {                /* a better match starts one byte later */
                    emit(&S, buf[i]);
                    have_cache = 1; cpos = i + 1; cl = l2; cd = d2;
                    i++;
                    continue;
                }
            }
            emit(&S, ((uint64_t)l << DBITS) | (uint64_t)(uint32_t)d);
            if (l >= SKIP_INSERT) {
                insert_to(&c, i + 4);
                c.ins = i + l;
            }
            i += l;
        } else {
            miss++;
            int step = 1 + (accel >= 31 ? 0 : (miss >> accel));
            if (step == 1) {
                emit(&S, buf[i]); i++;
            } else {
                if (step > MAX_STEP) step = MAX_STEP;
                int64_t e = i + step;
                if (e > n) e = n;
                while (i < e) { emit(&S, buf[i]); i++; }
            }
        }
    }
    int64_t nt = S.nt;
    LZ_FREE();
    *nruns_out = nr;
    return nt;
}

/* ------------------------------------------------------------ WAV filters */
/* 16-bit PCM. in = n frames of C interleaved little-endian samples; out = for
 * each channel a low-byte plane then a high-byte plane (n bytes each).
 * All arithmetic is mod 2^16, which is what the Python version amounts to. */
int kz_wav16_enc(const uint8_t *in, int64_t n, int C, int order, int ms, int bias,
                 uint8_t *out) {
    uint16_t *L = NULL, *t = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
    if (!t) return -1;
    if (ms) {
        L = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
        for (int64_t f = 0; f < n; f++)
            L[f] = (uint16_t)(in[2 * (f * C)] | (in[2 * (f * C) + 1] << 8));
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            const uint8_t *s = in + 2 * (f * C + c);
            t[f] = (uint16_t)(s[0] | (s[1] << 8));
        }
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (uint16_t)(t[f] - L[f]);
        for (int o = 0; o < order; o++)
            for (int64_t f = n - 1; f > 0; f--) t[f] = (uint16_t)(t[f] - t[f - 1]);
        uint8_t *lo = out + (int64_t)(2 * c) * n, *hi = lo + n;
        for (int64_t f = 0; f < n; f++) {
            uint16_t v = (uint16_t)(t[f] + bias);
            lo[f] = (uint8_t)v;
            hi[f] = (uint8_t)(v >> 8);
        }
    }
    free(t);
    free(L);
    return 0;
}

int kz_wav16_dec(const uint8_t *in, int64_t n, int C, int order, int ms, int bias,
                 uint8_t *out) {
    uint16_t *L = NULL, *t = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
    if (!t) return -1;
    if (ms) {
        L = (uint16_t *)malloc(sizeof(uint16_t) * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        const uint8_t *lo = in + (int64_t)(2 * c) * n, *hi = lo + n;
        for (int64_t f = 0; f < n; f++)
            t[f] = (uint16_t)((lo[f] | (hi[f] << 8)) - bias);
        for (int o = 0; o < order; o++)
            for (int64_t f = 1; f < n; f++) t[f] = (uint16_t)(t[f] + t[f - 1]);
        if (ms && c == 0) memcpy(L, t, sizeof(uint16_t) * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (uint16_t)(t[f] + L[f]);
        for (int64_t f = 0; f < n; f++) {
            uint8_t *d = out + 2 * (f * C + c);
            d[0] = (uint8_t)t[f];
            d[1] = (uint8_t)(t[f] >> 8);
        }
    }
    free(t);
    free(L);
    return 0;
}

/* 24/32-bit audio: byte planes only. plane index = channel * W + byte (LSB first) */
void kz_shuf_enc(const uint8_t *in, int64_t n, int W, int C, uint8_t *out) {
    for (int64_t f = 0; f < n; f++)
        for (int c = 0; c < C; c++) {
            const uint8_t *s = in + (f * C + c) * W;
            for (int k = 0; k < W; k++) out[(int64_t)(c * W + k) * n + f] = s[k];
        }
}

void kz_shuf_dec(const uint8_t *in, int64_t n, int W, int C, uint8_t *out) {
    for (int64_t f = 0; f < n; f++)
        for (int c = 0; c < C; c++) {
            uint8_t *d = out + (f * C + c) * W;
            for (int k = 0; k < W; k++) d[k] = in[(int64_t)(c * W + k) * n + f];
        }
}

/* Integer audio of W = 1, 3 or 4 bytes per sample (little-endian, interleaved).
 * out = for each channel W byte planes (LSB first) of n bytes each.
 * Per channel: (channel 1 minus channel 0 if ms), delta x order, zigzag; all mod 2^(8W). */
static uint32_t wmask(int W) { return W >= 4 ? 0xFFFFFFFFu : ((1u << (8 * W)) - 1u); }

int kz_int_enc(const uint8_t *in, int64_t n, int W, int C, int order, int ms, int zz,
               uint8_t *out) {
    uint32_t mask = wmask(W), sb = (uint32_t)(8 * W - 1);
    uint32_t *t = (uint32_t *)malloc(4 * (size_t)(n ? n : 1)), *L = NULL;
    if (!t) return -1;
    if (ms) {
        L = (uint32_t *)malloc(4 * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            const uint8_t *s = in + (f * C + c) * W;
            uint32_t v = 0;
            for (int k = 0; k < W; k++) v |= (uint32_t)s[k] << (8 * k);
            t[f] = v;
        }
        if (ms && c == 0) memcpy(L, t, 4 * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (t[f] - L[f]) & mask;
        for (int o = 0; o < order; o++)
            for (int64_t f = n - 1; f > 0; f--) t[f] = (t[f] - t[f - 1]) & mask;
        if (zz)
            for (int64_t f = 0; f < n; f++) {
                uint32_t v = t[f];
                t[f] = ((v << 1) & mask) ^ (((v >> sb) & 1u) ? mask : 0u);
            }
        for (int k = 0; k < W; k++) {
            uint8_t *pl = out + (int64_t)(c * W + k) * n;
            for (int64_t f = 0; f < n; f++) pl[f] = (uint8_t)(t[f] >> (8 * k));
        }
    }
    free(t);
    free(L);
    return 0;
}

int kz_int_dec(const uint8_t *in, int64_t n, int W, int C, int order, int ms, int zz,
               uint8_t *out) {
    uint32_t mask = wmask(W);
    uint32_t *t = (uint32_t *)malloc(4 * (size_t)(n ? n : 1)), *L = NULL;
    if (!t) return -1;
    if (ms) {
        L = (uint32_t *)malloc(4 * (size_t)(n ? n : 1));
        if (!L) { free(t); return -1; }
    }
    for (int c = 0; c < C; c++) {
        for (int64_t f = 0; f < n; f++) {
            uint32_t v = 0;
            for (int k = 0; k < W; k++)
                v |= (uint32_t)in[(int64_t)(c * W + k) * n + f] << (8 * k);
            t[f] = v;
        }
        if (zz)
            for (int64_t f = 0; f < n; f++) {
                uint32_t z = t[f];
                t[f] = ((z >> 1) ^ ((z & 1u) ? mask : 0u)) & mask;
            }
        for (int o = 0; o < order; o++)
            for (int64_t f = 1; f < n; f++) t[f] = (t[f] + t[f - 1]) & mask;
        if (ms && c == 0) memcpy(L, t, 4 * (size_t)n);
        if (ms && c == 1)
            for (int64_t f = 0; f < n; f++) t[f] = (t[f] + L[f]) & mask;
        for (int64_t f = 0; f < n; f++) {
            uint8_t *d = out + (f * C + c) * W;
            for (int k = 0; k < W; k++) d[k] = (uint8_t)(t[f] >> (8 * k));
        }
    }
    free(t);
    free(L);
    return 0;
}

/* float32 audio that is really integers: sample = k / 2^s, k in [-2^s, 2^s - 1].
 * kz_f2i returns -1 if any sample doesn't convert exactly (NaN, -0.0, fraction, range). */
int kz_f2i(const uint8_t *in, int64_t m, int s, int kw, uint8_t *out) {
    double S = (double)(1 << s);
    for (int64_t i = 0; i < m; i++) {
        const uint8_t *p = in + 4 * i;
        uint32_t u = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16)
                     | ((uint32_t)p[3] << 24);
        float x;
        if (u == 0x80000000u) return -1;
        memcpy(&x, &u, 4);
        double v = (double)x * S;
        if (!(v >= -S && v <= S - 1.0)) return -1;
        int32_t k = (int32_t)v;
        if ((double)k != v) return -1;
        uint32_t q = (uint32_t)k;
        for (int j = 0; j < kw; j++) out[i * kw + j] = (uint8_t)(q >> (8 * j));
    }
    return 0;
}

void kz_i2f(const uint8_t *in, int64_t m, int s, int kw, uint8_t *out) {
    double S = (double)(1 << s);
    int sh = 32 - 8 * kw;
    for (int64_t i = 0; i < m; i++) {
        uint32_t v = 0;
        for (int j = 0; j < kw; j++) v |= (uint32_t)in[i * kw + j] << (8 * j);
        int32_t k = (int32_t)(v << sh) >> sh;
        float x = (float)((double)k / S);
        uint32_t u;
        memcpy(&u, &x, 4);
        for (int j = 0; j < 4; j++) out[i * 4 + j] = (uint8_t)(u >> (8 * j));
    }
}

/* ------------------------------------------------------------ chunk classes */
#define RND_ENTROPY 7.95
#define LIT_ENTROPY 0.6
#define EASY_ENTROPY 3.0

static double log2_(double x) {
    /* tiny log2 without libm: exponent + polynomial on the mantissa */
    union { double d; uint64_t u; } v;
    v.d = x;
    int e = (int)((v.u >> 52) & 0x7FF) - 1023;
    v.u = (v.u & 0x000FFFFFFFFFFFFFULL) | 0x3FF0000000000000ULL;
    double m = v.d, t = (m - 1.0) / (m + 1.0), t2 = t * t;
    double ln = 2.0 * t * (1.0 + t2 * (1.0 / 3 + t2 * (1.0 / 5 + t2 * (1.0 / 7 + t2 / 9))));
    return e + ln * 1.4426950408889634;
}

/* count distinct g-byte groups in s[0..len) with a small open-addressing set;
 * returns the number of distinct groups */
static int distinct_grams_m(const uint8_t *s, int len, int g, uint32_t *tab, uint32_t *used, uint32_t M) {
    int ng = len - g + 1, uniq = 0;
    memset(used, 0, M / 8);
    for (int i = 0; i < ng; i++) {
        uint64_t h = 1469598103934665603ULL;
        for (int k = 0; k < g; k++) h = (h ^ s[i + k]) * 1099511628211ULL;
        uint32_t slot = (uint32_t)(h >> 40) & (M - 1);
        for (;;) {
            if (!(used[slot >> 5] & (1u << (slot & 31)))) {
                used[slot >> 5] |= 1u << (slot & 31);
                tab[slot] = (uint32_t)i;
                uniq++;
                break;
            }
            if (memcmp(s + tab[slot], s + i, (size_t)g) == 0) break;
            slot = (slot + 1) & (M - 1);
        }
    }
    return uniq;
}

static int distinct_grams(const uint8_t *s, int len, int g, uint32_t *tab, uint32_t *used) {
    return distinct_grams_m(s, len, g, tab, used, 16384);
}

/* entropy of the byte given a context out of 8 (cnt[ctx*256 + byte]) over n bytes */
static double cond_ent(const int *cnt, int n) {
    double acc = 0.0;
    for (int c = 0; c < 8; c++) {
        int tot = 0;
        for (int v = 0; v < 256; v++) tot += cnt[c * 256 + v];
        if (!tot) continue;
        double h = 0.0, it = 1.0 / tot;
        for (int v = 0; v < 256; v++)
            if (cnt[c * 256 + v]) { double p = cnt[c * 256 + v] * it; h -= p * log2_(p); }
        acc += (double)tot / n * h;
    }
    return acc;
}

/* class per 64 KB chunk: 0 normal, 1 random, 2 literal-only, 3 sparse */
void kz_rnd(const uint8_t *buf, int64_t n, uint8_t *rnd) {
    int64_t nchunks = (n + (1 << RND_BITS) - 1) >> RND_BITS;
    uint8_t s[8192];
    uint32_t *tab = (uint32_t *)malloc(sizeof(uint32_t) * 16384);
    uint32_t *used = (uint32_t *)malloc(16384 / 8);
    uint32_t *tabB = (uint32_t *)malloc(sizeof(uint32_t) * 262144);
    uint32_t *usedB = (uint32_t *)malloc(262144 / 8);
    for (int64_t ch = 0; ch < nchunks; ch++) {
        rnd[ch] = 0;
        int64_t a = ch << RND_BITS;
        int len = 0;
        for (int k = 0; k < 8; k++) {
            int64_t from = a + k * 8192;
            if (from >= n) break;
            int64_t cnt = n - from < 1024 ? n - from : 1024;
            memcpy(s + len, buf + from, (size_t)cnt);
            len += (int)cnt;
        }
        if (len < 4096 || !tab || !used) continue;
        int freq[256] = {0};
        for (int i = 0; i < len; i++) freq[s[i]]++;
        double ent = 0.0, inv = 1.0 / len;
        for (int v = 0; v < 256; v++)
            if (freq[v]) { double p = freq[v] * inv; ent -= p * log2_(p); }
        if (ent >= RND_ENTROPY) {
            int u = distinct_grams(s, len, 4, tab, used);
            rnd[ch] = (u >= 0.99 * (len - 3)) ? 1 : 0;
        } else if (ent >= LIT_ENTROPY) {
            if (ent >= 3.0 && tabB && usedB) {
                /* whole-chunk test. Noise passes only if (1) knowing the byte's position
                 * in an 8-byte period or the previous byte's top bits tells (almost)
                 * nothing, and (2) it holds no more repeated groups than chance gives. */
                int clen = (int)(n - a < (1 << RND_BITS) ? n - a : (1 << RND_BITS));
                int64_t fq[256] = {0};
                static int ph[8 * 256], pv[8 * 256];
                memset(ph, 0, sizeof ph);
                memset(pv, 0, sizeof pv);
                for (int i = 0; i < clen; i++) {
                    int x = buf[a + i];
                    fq[x]++;
                    ph[(i & 7) * 256 + x]++;
                    pv[(i ? buf[a + i - 1] >> 5 : 0) * 256 + x]++;
                }
                double p2 = 0.0, ic = 1.0 / clen, h0 = 0.0;
                for (int v = 0; v < 256; v++) {
                    double p = fq[v] * ic;
                    p2 += p * p;
                    if (p > 0) h0 -= p * log2_(p);
                }
                double gap1 = h0 - cond_ent(ph, clen), gap2 = h0 - cond_ent(pv, clen);
                if (gap1 < 0.06 && gap2 < 0.06) {
                    int ng_ = 4;
                    double expd = 0.0;
                    for (int g = 4; g <= 16; g++) {
                        double q = 1.0;
                        for (int k = 0; k < g; k++) q *= p2;
                        expd = 0.5 * (double)(clen - g + 1) * (double)(clen - g + 1) * q;
                        ng_ = g;
                        if (expd <= 0.1 * (clen - g + 1)) break;
                    }
                    int ng = clen - ng_ + 1;
                    if (ng > 0 && expd <= 0.1 * ng) {
                        int u = distinct_grams_m(buf + a, clen, ng_, tabB, usedB, 262144);
                        double dup = (double)(ng - u);
                        if (dup <= 1.5 * expd + 0.004 * ng) rnd[ch] = 2;
                    }
                }
            } else {
                int g = (int)(32 / ent) + 1;
                if (g > 64) g = 64;
                int u = distinct_grams(s, len, g, tab, used);
                if (u >= 0.99 * (len - g + 1)) rnd[ch] = 2;
                else if (ent < EASY_ENTROPY) rnd[ch] = 3;
            }
        }
    }
    free(tab);
    free(used);
    free(tabB);
    free(usedB);
}
