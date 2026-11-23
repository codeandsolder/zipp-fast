"""codecs registry and standard encoding helpers for Zipp Python."""
import builtins

BOM = b"\xff\xfe"
BOM_UTF8 = b"\xef\xbb\xbf"
BOM_LE = b"\xff\xfe"
BOM_BE = b"\xfe\xff"
BOM_UTF16 = b"\xff\xfe"
BOM_UTF16_LE = b"\xff\xfe"
BOM_UTF16_BE = b"\xfe\xff"
BOM_UTF32 = b"\xff\xfe\x00\x00"
BOM_UTF32_LE = b"\xff\xfe\x00\x00"
BOM_UTF32_BE = b"\x00\x00\xfe\xff"
BOM32_LE = BOM_UTF16_LE
BOM32_BE = BOM_UTF16_BE
BOM64_LE = BOM_UTF32_LE
BOM64_BE = BOM_UTF32_BE

_search_functions = []
_error_handlers = {}


def _norm_enc(encoding):
    return encoding.lower().replace("_", "-").strip()


def _utf16_decode(data, errors="strict", byteorder=0):
    if not isinstance(data, (bytes, bytearray)):
        data = bytes(data)
    start = 0
    if byteorder == 0:
        if data[:2] == BOM_UTF16_LE:
            byteorder = -1
            start = 2
        elif data[:2] == BOM_UTF16_BE:
            byteorder = 1
            start = 2
        else:
            byteorder = -1
    out = []
    i = start
    n = len(data)
    while i + 1 < n:
        if byteorder < 0:
            u = data[i] | (data[i + 1] << 8)
        else:
            u = (data[i] << 8) | data[i + 1]
        i += 2
        if 0xD800 <= u <= 0xDBFF and i + 1 < n:
            if byteorder < 0:
                u2 = data[i] | (data[i + 1] << 8)
            else:
                u2 = (data[i] << 8) | data[i + 1]
            if 0xDC00 <= u2 <= 0xDFFF:
                i += 2
                out.append(chr(0x10000 + ((u - 0xD800) << 10) + (u2 - 0xDC00)))
                continue
        out.append(chr(u))
    return "".join(out), i


def _utf16_encode(text, errors="strict", byteorder=0):
    out = bytearray()
    if byteorder == 0:
        out.extend(BOM_UTF16_LE)
        byteorder = -1
    for ch in text:
        cp = ord(ch)
        if cp < 0x10000:
            units = (cp,)
        else:
            v = cp - 0x10000
            units = (0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF))
        for u in units:
            if byteorder < 0:
                out.append(u & 0xFF)
                out.append((u >> 8) & 0xFF)
            else:
                out.append((u >> 8) & 0xFF)
                out.append(u & 0xFF)
    return bytes(out), len(text)


def _utf8_sig_decode(data, errors="strict"):
    if not isinstance(data, (bytes, bytearray)):
        data = bytes(data)
    raw = data[3:] if data[:3] == BOM_UTF8 else data
    return raw.decode("utf-8", errors), len(data)


def _utf8_sig_encode(text, errors="strict"):
    return BOM_UTF8 + text.encode("utf-8", errors), len(text)


def utf_8_decode(data, errors="strict", final=True):
    return (data if isinstance(data, bytes) else bytes(data)).decode("utf-8", errors), len(data)


def utf_8_encode(text, errors="strict"):
    return text.encode("utf-8", errors), len(text)


def utf_16_decode(data, errors="strict", final=True):
    return _utf16_decode(data, errors, 0)


def utf_16_le_decode(data, errors="strict", final=True):
    return _utf16_decode(data, errors, -1)


def utf_16_be_decode(data, errors="strict", final=True):
    return _utf16_decode(data, errors, 1)


def utf_16_encode(text, errors="strict"):
    return _utf16_encode(text, errors, 0)


def utf_16_le_encode(text, errors="strict"):
    return _utf16_encode(text, errors, -1)


def utf_16_be_encode(text, errors="strict"):
    return _utf16_encode(text, errors, 1)


def latin_1_decode(data, errors="strict"):
    return (data if isinstance(data, bytes) else bytes(data)).decode("latin-1", errors), len(data)


def latin_1_encode(text, errors="strict"):
    return text.encode("latin-1", errors), len(text)


def ascii_decode(data, errors="strict"):
    return (data if isinstance(data, bytes) else bytes(data)).decode("ascii", errors), len(data)


def ascii_encode(text, errors="strict"):
    return text.encode("ascii", errors), len(text)


class IncrementalEncoder:
    def __init__(self, errors="strict"):
        self.errors = errors

    def encode(self, object, final=False):
        raise NotImplementedError

    def reset(self):
        pass


class IncrementalDecoder:
    def __init__(self, errors="strict"):
        self.errors = errors

    def decode(self, object, final=False):
        raise NotImplementedError

    def reset(self):
        pass


class StreamWriter:
    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors


class StreamReader:
    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors


class CodecInfo(tuple):
    def __new__(cls, encode, decode, streamreader=None, streamwriter=None, incrementalencoder=None, incrementaldecoder=None, name=None, *, _is_text_encoding=True):
        self = tuple.__new__(cls, (encode, decode, streamreader, streamwriter))
        self.name = name
        self.encode = encode
        self.decode = decode
        self.incrementalencoder = incrementalencoder
        self.incrementaldecoder = incrementaldecoder
        self.streamwriter = streamwriter
        self.streamreader = streamreader
        self._is_text_encoding = _is_text_encoding
        return self


_CODECS = {
    "utf-8": ("utf-8", utf_8_encode, utf_8_decode),
    "utf8": ("utf-8", utf_8_encode, utf_8_decode),
    "u8": ("utf-8", utf_8_encode, utf_8_decode),
    "utf-8-sig": ("utf-8-sig", _utf8_sig_encode, _utf8_sig_decode),
    "utf-16": ("utf-16", utf_16_encode, utf_16_decode),
    "utf16": ("utf-16", utf_16_encode, utf_16_decode),
    "utf-16-le": ("utf-16-le", utf_16_le_encode, utf_16_le_decode),
    "utf-16le": ("utf-16-le", utf_16_le_encode, utf_16_le_decode),
    "utf-16-be": ("utf-16-be", utf_16_be_encode, utf_16_be_decode),
    "utf-16be": ("utf-16-be", utf_16_be_encode, utf_16_be_decode),
    "ascii": ("ascii", ascii_encode, ascii_decode),
    "us-ascii": ("ascii", ascii_encode, ascii_decode),
    "latin-1": ("iso8859-1", latin_1_encode, latin_1_decode),
    "latin1": ("iso8859-1", latin_1_encode, latin_1_decode),
    "iso-8859-1": ("iso8859-1", latin_1_encode, latin_1_decode),
    "iso8859-1": ("iso8859-1", latin_1_encode, latin_1_decode),
    "cp1252": ("cp1252", latin_1_encode, latin_1_decode),
    "windows-1252": ("cp1252", latin_1_encode, latin_1_decode),
}


def register(search_function):
    if not callable(search_function):
        raise TypeError("argument must be callable")
    _search_functions.append(search_function)


def register_error(name, error_handler):
    _error_handlers[name] = error_handler


def lookup_error(name):
    if name in _error_handlers:
        return _error_handlers[name]
    raise LookupError("unknown error handler name %r" % (name,))


def lookup(encoding):
    norm = _norm_enc(encoding)
    for fn in _search_functions:
        info = fn(norm)
        if info is not None:
            return info
    entry = _CODECS.get(norm)
    if entry is None:
        raise LookupError("unknown encoding: %s" % encoding)
    cname, enc_fn, dec_fn = entry
    return CodecInfo(enc_fn, dec_fn, StreamReader, StreamWriter, IncrementalEncoder, IncrementalDecoder, name=cname)


def encode(obj, encoding="utf-8", errors="strict"):
    info = lookup(encoding)
    return info.encode(obj, errors)[0]


def decode(obj, encoding="utf-8", errors="strict"):
    info = lookup(encoding)
    return info.decode(obj, errors)[0]


def getencoder(encoding):
    return lookup(encoding).encode


def getdecoder(encoding):
    return lookup(encoding).decode


def open(filename, mode="r", encoding=None, errors="strict", buffering=-1):
    return builtins.open(filename, mode, buffering=buffering, encoding=encoding, errors=errors)
