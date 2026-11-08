#!/usr/bin/env python3
"""Writes the two golden `.skln` v1 files next to this script, and checks them.

This is a second, independent implementation of docs/FORMAT.md ("The .skln container"), written from
the specification and not from the Rust code, using the `cryptography` package. It exists so the
format is pinned by something other than the code that wrote it: the Rust test in
`skln/tests/golden.rs` must open these files.

The files were created once, with fixed keys and salts so they are reproducible. Never regenerate
them to make a failing test pass: a change that breaks them broke version 1, so bump the version.

    pip install cryptography
    python3 make_v1.py            # rewrite the golden files
    python3 make_v1.py --verify   # decrypt the committed files and compare with the expected contents
"""
import struct
import sys
from pathlib import Path

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

MAGIC = b"\x89SKLN\r\n\x1a"
HEADER_LEN = 36
PASSWORD_BLOCK_LEN = 26
TAG_LEN = 16
KEY = bytes(range(32))
HERE = Path(__file__).resolve().parent


def file_key(key: bytes, salt: bytes) -> bytes:
    return HKDF(
        algorithm=hashes.SHA256(), length=32, salt=salt, info=b"kiln skln v1 file key"
    ).derive(key)


def seal(key, kind, chunk_log2, revision, salt, plaintext, password_block=None):
    flags = 1 if password_block is not None else 0
    header = MAGIC + bytes([1, flags, kind, chunk_log2]) + struct.pack("<Q", revision) + salt
    if password_block is not None:
        assert len(password_block) == PASSWORD_BLOCK_LEN
        header += password_block
    chunk = 1 << chunk_log2
    pieces = [plaintext[i : i + chunk] for i in range(0, len(plaintext), chunk)] or [b""]
    cipher = ChaCha20Poly1305(file_key(key, salt))
    out = bytearray(header)
    for i, piece in enumerate(pieces):
        last = i == len(pieces) - 1
        nonce = b"\0\0\0\0" + struct.pack("<Q", i)
        ct = cipher.encrypt(nonce, piece, header + (b"\x01" if last else b"\x00"))
        out += struct.pack("<I", len(ct)) + ct
    return bytes(out)


def open_file(key, data):
    assert data[:8] == MAGIC and data[8] == 1
    flags, kind, chunk_log2 = data[9], data[10], data[11]
    assert flags & ~1 == 0 and 12 <= chunk_log2 <= 24
    (revision,) = struct.unpack("<Q", data[12:20])
    salt = data[20:36]
    header_len = HEADER_LEN + (PASSWORD_BLOCK_LEN if flags & 1 else 0)
    header = data[:header_len]
    cipher = ChaCha20Poly1305(file_key(key, salt))
    full = (1 << chunk_log2) + TAG_LEN
    pos, i, out = header_len, 0, bytearray()
    while True:
        (n,) = struct.unpack("<I", data[pos : pos + 4])
        body = data[pos + 4 : pos + 4 + n]
        assert len(body) == n
        last = pos + 4 + n == len(data)
        assert (TAG_LEN <= n <= full) if last else n == full
        nonce = b"\0\0\0\0" + struct.pack("<Q", i)
        out += cipher.decrypt(nonce, body, header + (b"\x01" if last else b"\x00"))
        if last:
            return kind, revision, bytes(out)
        pos += 4 + n
        i += 1


def pack_bundle(members):
    out = struct.pack("<I", len(members))
    for name, data in members:
        n = name.encode()
        out += struct.pack("<H", len(n)) + n + struct.pack("<I", len(data)) + data
    return out


def unpack_bundle(b):
    (count,) = struct.unpack("<I", b[:4])
    pos, out = 4, []
    for _ in range(count):
        (nl,) = struct.unpack("<H", b[pos : pos + 2])
        name = b[pos + 2 : pos + 2 + nl].decode()
        pos += 2 + nl
        (dl,) = struct.unpack("<I", b[pos : pos + 4])
        out.append((name, b[pos + 4 : pos + 4 + dl]))
        pos += 4 + dl
    assert pos == len(b)
    return out


# The same contents are rebuilt in skln/tests/golden.rs.
BUNDLE_MEMBERS = [
    ("vault.json", "".join(f"line {i}\n" for i in range(1, 1501)).encode()),
    ("readme.txt", b"Silk vault golden file\n"),
    ("empty", b""),
]
ARCHIVE_PAYLOAD = b"hello, skln"
PASSWORD_BLOCK = bytes([1, 17]) + struct.pack("<II", 8, 1) + bytes(range(0xA0, 0xB0))

BUNDLE = dict(
    name="v1-bundle.skln",
    data=lambda: seal(KEY, 1, 12, 42, bytes(range(100, 116)), pack_bundle(BUNDLE_MEMBERS)),
)
ARCHIVE = dict(
    name="v1-archive-password-block.skln",
    data=lambda: seal(
        KEY, 0, 20, 0, bytes(range(200, 216)), ARCHIVE_PAYLOAD, password_block=PASSWORD_BLOCK
    ),
)


def verify():
    kind, rev, plain = open_file(KEY, (HERE / BUNDLE["name"]).read_bytes())
    assert (kind, rev) == (1, 42) and unpack_bundle(plain) == BUNDLE_MEMBERS
    kind, rev, plain = open_file(KEY, (HERE / ARCHIVE["name"]).read_bytes())
    assert (kind, rev, plain) == (0, 0, ARCHIVE_PAYLOAD)
    print("golden files decrypt and match the expected contents")



if __name__ == "__main__":
    if "--verify" in sys.argv:
        verify()
    else:
        for f in (BUNDLE, ARCHIVE):
            (HERE / f["name"]).write_bytes(f["data"]())
            print("wrote", f["name"], (HERE / f["name"]).stat().st_size, "bytes")
        verify()
