"""Binary/ASCII conversions (`binascii`) for Zipp Python."""
import zlib


class Error(ValueError):
    pass


class Incomplete(Exception):
    pass


_B64_CHARS = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
_B64_MAP = {c: i for i, c in enumerate(_B64_CHARS)}


def _as_bytes(data):
    if isinstance(data, bytes):
        return data
    if isinstance(data, bytearray):
        return bytes(data)
    if isinstance(data, str):
        try:
            return data.encode("ascii")
        except Exception:
            raise ValueError("string argument should contain only ASCII characters")
    raise TypeError("a bytes-like object is required, not %r" % type(data).__name__)


def hexlify(data, sep=None, bytes_per_sep=1):
    b = _as_bytes(data)
    h = b.hex()
    if sep is None:
        return h.encode("ascii")
    s = sep.decode("ascii") if isinstance(sep, bytes) else str(sep)
    if len(s) != 1:
        raise ValueError("sep must be length 1.")
    out = [h[i:i + 2] for i in range(0, len(h), 2)]
    return s.join(out).encode("ascii")


b2a_hex = hexlify


def unhexlify(hexstr):
    b = _as_bytes(hexstr)
    if len(b) % 2 != 0:
        raise Error("Odd-length string")
    try:
        return bytes.fromhex(b.decode("ascii"))
    except ValueError as e:
        raise Error(str(e))


a2b_hex = unhexlify


def b2a_base64(data, *, newline=True):
    b = _as_bytes(data)
    out = bytearray()
    n = len(b)
    i = 0
    while i + 3 <= n:
        v = (b[i] << 16) | (b[i + 1] << 8) | b[i + 2]
        out.append(_B64_CHARS[(v >> 18) & 63])
        out.append(_B64_CHARS[(v >> 12) & 63])
        out.append(_B64_CHARS[(v >> 6) & 63])
        out.append(_B64_CHARS[v & 63])
        i += 3
    rem = n - i
    if rem == 1:
        v = b[i] << 16
        out.append(_B64_CHARS[(v >> 18) & 63])
        out.append(_B64_CHARS[(v >> 12) & 63])
        out.append(61)
        out.append(61)
    elif rem == 2:
        v = (b[i] << 16) | (b[i + 1] << 8)
        out.append(_B64_CHARS[(v >> 18) & 63])
        out.append(_B64_CHARS[(v >> 12) & 63])
        out.append(_B64_CHARS[(v >> 6) & 63])
        out.append(61)
    if newline:
        out.append(10)
    return bytes(out)


def a2b_base64(data, *, strict_mode=False):
    b = _as_bytes(data)
    vals = []
    pad = 0
    for c in b:
        if c in _B64_MAP:
            if pad and strict_mode:
                raise Error("Excess data after padding")
            if pad:
                break
            vals.append(_B64_MAP[c])
        elif c == 61:
            pad += 1
        elif strict_mode:
            raise Error("Only base64 data is allowed")
    if strict_mode and len(vals) % 4 == 1:
        raise Error("Invalid base64-encoded string")
    out = bytearray()
    i = 0
    n = len(vals)
    while i + 4 <= n:
        v = (vals[i] << 18) | (vals[i + 1] << 12) | (vals[i + 2] << 6) | vals[i + 3]
        out.append((v >> 16) & 0xFF)
        out.append((v >> 8) & 0xFF)
        out.append(v & 0xFF)
        i += 4
    rem = n - i
    if rem == 2:
        v = (vals[i] << 18) | (vals[i + 1] << 12)
        out.append((v >> 16) & 0xFF)
    elif rem == 3:
        v = (vals[i] << 18) | (vals[i + 1] << 12) | (vals[i + 2] << 6)
        out.append((v >> 16) & 0xFF)
        out.append((v >> 8) & 0xFF)
    elif rem == 1:
        raise Error("Invalid base64-encoded string: number of data characters cannot be 1 more than a multiple of 4")
    return bytes(out)


def crc32(data, value=0):
    return zlib.crc32(_as_bytes(data), value)
