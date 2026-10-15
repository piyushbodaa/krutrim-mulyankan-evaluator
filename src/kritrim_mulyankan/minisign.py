"""Verify minisign / Tauri update signatures with the standard library only.

Ed25519 verification follows RFC 8032 section 5.1.7. It is slow pure Python, which is
fine for one checksum file per release. Only verification is implemented: the signing
key never leaves the release workflow.
"""

from __future__ import annotations

import base64
import hashlib

_P = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_I = pow(2, (_P - 1) // 4, _P)


def _recover_x(y: int, sign: int) -> int | None:
    if y >= _P:
        return None
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _I % _P
    if (x * x - x2) % _P:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


_GY = 4 * pow(5, _P - 2, _P) % _P
_G = (_recover_x(_GY, 0), _GY, 1, _recover_x(_GY, 0) * _GY % _P)


def _add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    aa = (y1 - x1) * (y2 - x2) % _P
    bb = (y1 + x1) * (y2 + x2) % _P
    cc = 2 * t1 * t2 * _D % _P
    dd = 2 * z1 * z2 % _P
    e, f, g, h = bb - aa, dd - cc, dd + cc, bb + aa
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _mul(scalar: int, point):
    result = (0, 1, 1, 0)
    while scalar:
        if scalar & 1:
            result = _add(result, point)
        point = _add(point, point)
        scalar >>= 1
    return result


def _equal(a, b) -> bool:
    return (a[0] * b[2] - b[0] * a[2]) % _P == 0 and (a[1] * b[2] - b[1] * a[2]) % _P == 0


def _decompress(data: bytes):
    if len(data) != 32:
        return None
    y = int.from_bytes(data, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


def ed25519_verify(public: bytes, message: bytes, signature: bytes) -> bool:
    if len(public) != 32 or len(signature) != 64:
        return False
    a = _decompress(public)
    r = _decompress(signature[:32])
    s = int.from_bytes(signature[32:], "little")
    if a is None or r is None or s >= _L:
        return False
    h = int.from_bytes(hashlib.sha512(signature[:32] + public + message).digest(), "little") % _L
    return _equal(_mul(s, _G), _add(r, _mul(h, a)))


def verify(data: bytes, signature_b64: str, public_key_b64: str) -> bool:
    """True when `signature_b64` (a Tauri .sig: base64 of a minisign signature file)
    signs `data` with `public_key_b64` (the key line of a minisign public key)."""
    try:
        lines = base64.b64decode(signature_b64.strip()).decode().splitlines()
        sig = base64.b64decode(lines[1])
        trusted = lines[2].removeprefix("trusted comment: ").encode()
        global_sig = base64.b64decode(lines[3])
        key = base64.b64decode(public_key_b64)
    except (ValueError, IndexError, UnicodeDecodeError):
        return False
    if len(sig) != 74 or len(key) != 42 or sig[2:10] != key[2:10] or key[:2] != b"Ed":
        return False
    algorithm, signed, public = sig[:2], sig[10:], key[10:]
    if algorithm == b"ED":
        message = hashlib.blake2b(data, digest_size=64).digest()
    elif algorithm == b"Ed":
        message = data
    else:
        return False
    return ed25519_verify(public, message, signed) and ed25519_verify(
        public, signed + trusted, global_sig
    )
