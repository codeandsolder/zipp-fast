"""RFC 4648 Base16, Base32, Base64 data encodings (`base64`) for Zipp Python."""
import binascii

__all__ = [
    "b64encode", "b64decode", "standard_b64encode", "standard_b64decode",
    "urlsafe_b64encode", "urlsafe_b64decode", "b32encode", "b32decode",
    "b16encode", "b16decode", "encodebytes", "decodebytes",
]


def _bytes_from_decode_data(s):
    if isinstance(s, str):
        try:
            return s.encode("ascii")
        except Exception:
            raise ValueError("string argument should contain only ASCII characters")
    if isinstance(s, (bytes, bytearray)):
        return bytes(s)
    raise TypeError("argument should be a bytes-like object or ASCII string, not %r" % type(s).__name__)


def b64encode(s, altchars=None):
    encoded = binascii.b2a_base64(s, newline=False)
    if altchars is not None:
        alt = _bytes_from_decode_data(altchars)
        if len(alt) != 2:
            raise ValueError("altchars must be length 2")
        encoded = encoded.replace(b"+", alt[0:1]).replace(b"/", alt[1:2])
    return encoded


def b64decode(s, altchars=None, validate=False):
    b = _bytes_from_decode_data(s)
    if altchars is not None:
        alt = _bytes_from_decode_data(altchars)
        if len(alt) != 2:
            raise ValueError("altchars must be length 2")
        b = b.replace(alt[0:1], b"+").replace(alt[1:2], b"/")
    return binascii.a2b_base64(b, strict_mode=validate)


def standard_b64encode(s):
    return b64encode(s)


def standard_b64decode(s):
    return b64decode(s)


def urlsafe_b64encode(s):
    return b64encode(s, altchars=b"-_")


def urlsafe_b64decode(s):
    return b64decode(s, altchars=b"-_")


def encodebytes(s):
    if not isinstance(s, (bytes, bytearray)):
        raise TypeError("expected bytes-like object")
    b = bytes(s)
    pieces = []
    for i in range(0, len(b), 57):
        pieces.append(binascii.b2a_base64(b[i:i + 57], newline=True))
    return b"".join(pieces)


def decodebytes(s):
    return binascii.a2b_base64(s)


def b16encode(s):
    return binascii.hexlify(s).upper()


def b16decode(s, casefold=False):
    b = _bytes_from_decode_data(s)
    if casefold:
        b = b.upper()
    return binascii.unhexlify(b)


_B32_ALPHA = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
_B32_MAP = {c: i for i, c in enumerate(_B32_ALPHA)}


def b32encode(s):
    b = bytes(s)
    out = bytearray()
    for i in range(0, len(b), 5):
        chunk = b[i:i + 5]
        n = len(chunk)
        padded = chunk + b"\x00" * (5 - n)
        v = (padded[0] << 32) | (padded[1] << 24) | (padded[2] << 16) | (padded[3] << 8) | padded[4]
        chars = [_B32_ALPHA[(v >> shift) & 31] for shift in (35, 30, 25, 20, 15, 10, 5, 0)]
        keep = {1: 2, 2: 4, 3: 5, 4: 7, 5: 8}[n]
        for j in range(keep):
            out.append(chars[j])
        for _ in range(8 - keep):
            out.append(61)
    return bytes(out)


def b32decode(s, casefold=False, map01=None):
    b = _bytes_from_decode_data(s)
    if casefold:
        b = b.upper()
    if map01 is not None:
        m = _bytes_from_decode_data(map01)
        b = b.replace(b"0", b"O").replace(b"1", m[0:1])
    if len(b) % 8 != 0:
        raise binascii.Error("Incorrect padding")
    out = bytearray()
    for i in range(0, len(b), 8):
        chunk = b[i:i + 8]
        pad = 0
        vals = []
        for c in chunk:
            if c == 61:
                pad += 1
                vals.append(0)
            elif c in _B32_MAP:
                vals.append(_B32_MAP[c])
            else:
                raise binascii.Error("Non-base32 digit found")
        v = 0
        for x in vals:
            v = (v << 5) | x
        nbytes = {0: 5, 1: 4, 3: 3, 4: 2, 6: 1}.get(pad)
        if nbytes is None:
            raise binascii.Error("Incorrect padding")
        for shift in (32, 24, 16, 8, 0)[:nbytes]:
            out.append((v >> shift) & 0xFF)
    return bytes(out)
