"""zipfile for Zipp: reading and writing ZIP archives with ZIP_STORED and
ZIP_DEFLATED entries (honoring compress_type, compresslevel, and ZipInfo
metadata). Mode "a" appends to an existing archive (its existing entries are
kept byte for byte) and mode "x" refuses to replace an existing file."""
import os
import struct
import zlib

ZIP_STORED = 0
ZIP_DEFLATED = 8
ZIP_BZIP2 = 12
ZIP_LZMA = 14


class BadZipFile(Exception):
    pass


BadZipfile = BadZipFile


def _check_compression(compression):
    if compression in (ZIP_STORED, ZIP_DEFLATED):
        return
    if compression in (ZIP_BZIP2, ZIP_LZMA):
        raise NotImplementedError("That compression method is not supported")
    raise RuntimeError("That compression method is not supported")


def _encode_dos_datetime(date_time):
    year, month, day, hour, minute, second = date_time
    if year < 1980:
        raise ValueError("ZIP does not support timestamps before 1980")
    dosdate = ((year - 1980) << 9) | (month << 5) | day
    dostime = (hour << 11) | (minute << 5) | (second // 2)
    return dostime & 0xFFFF, dosdate & 0xFFFF


def _decode_dos_datetime(dostime, dosdate):
    year = ((dosdate >> 9) & 0x7F) + 1980
    month = (dosdate >> 5) & 0x0F
    day = dosdate & 0x1F
    hour = (dostime >> 11) & 0x1F
    minute = (dostime >> 5) & 0x3F
    second = (dostime & 0x1F) * 2
    return (year, month, day, hour, minute, second)


class ZipInfo:
    def __init__(self, filename="NoName", date_time=(1980, 1, 1, 0, 0, 0)):
        self.orig_filename = filename
        null_byte = filename.find("\x00")
        if null_byte >= 0:
            filename = filename[:null_byte]
        self.filename = filename
        self.date_time = tuple(date_time)
        if self.date_time[0] < 1980:
            raise ValueError("ZIP does not support timestamps before 1980")
        self.compress_type = ZIP_STORED
        self._compresslevel = None
        self.comment = b""
        self.extra = b""
        self.create_system = 3
        self.create_version = 20
        self.extract_version = 20
        self.flag_bits = 0
        self.volume = 0
        self.internal_attr = 0
        self.external_attr = 0
        self.file_size = 0
        self.compress_size = 0
        self.header_offset = 0
        self.CRC = 0

    def is_dir(self):
        return self.filename.endswith("/")

    def __repr__(self):
        return "<ZipInfo filename=%r compress_type=%r file_size=%d>" % (
            self.filename,
            self.compress_type,
            self.file_size,
        )


def _crc32(data):
    return zlib.crc32(data if isinstance(data, bytes) else bytes(data))


def _concat(pieces):
    pieces = [p for p in pieces if p]
    if not pieces:
        return b""
    while len(pieces) > 1:
        merged = [pieces[i] + pieces[i + 1] for i in range(0, len(pieces) - 1, 2)]
        if len(pieces) % 2:
            merged.append(pieces[-1])
        pieces = merged
    return pieces[0]


_EOCD_SEARCH = 22 + 65535


class ZipFile:
    def __init__(self, file, mode="r", compression=ZIP_STORED, allowZip64=True, compresslevel=None, *, strict_timestamps=True):
        if mode not in ("r", "w", "x", "a"):
            raise ValueError("ZipFile requires mode 'r', 'w', 'x', or 'a'")
        _check_compression(compression)
        self.mode = mode
        self.compression = compression
        self.compresslevel = compresslevel
        self.filename = None
        self._entries = []
        self._data = b""
        self._closed = False
        self._file = None
        self._prefix = b""
        self._records = []
        if hasattr(file, "__fspath__"):
            file = file.__fspath__()
        if mode == "r":
            if isinstance(file, (bytes, bytearray)):
                self._data = bytes(file)
            elif hasattr(file, "read"):
                self._data = file.read()
            else:
                self.filename = str(file)
                with open(self.filename, "rb") as f:
                    self._data = f.read()
            self._read_directory()
            return
        if hasattr(file, "write"):
            self._file = file
            if mode == "a" and hasattr(file, "read") and hasattr(file, "seek"):
                file.seek(0)
                self._append_to(file.read())
                file.seek(0)
                if hasattr(file, "truncate"):
                    file.truncate()
            return
        self.filename = str(file)
        exists = os.path.exists(self.filename)
        if mode == "x" and exists:
            raise FileExistsError("[Errno 17] File exists: %r" % (self.filename,))
        if mode == "a" and exists:
            with open(self.filename, "rb") as f:
                self._append_to(f.read())

    def _append_to(self, data):
        if not data:
            return
        self._data = data
        try:
            start = self._read_directory()
        except BadZipFile:
            self._prefix = data
            self._entries = []
            self._data = b""
            return
        self._prefix = data[:start]

    def _read_directory(self):
        data = self._data
        tail = max(0, len(data) - _EOCD_SEARCH)
        end = data[tail:].rfind(b"PK\x05\x06")
        if end < 0:
            raise BadZipFile("File is not a zip file")
        end += tail
        count = struct.unpack("<H", data[end + 10:end + 12])[0]
        offset = struct.unpack("<I", data[end + 16:end + 20])[0]
        if (offset == 0xFFFFFFFF or count == 0xFFFF) and end >= 20 and data[end - 20:end - 16] == b"PK\x06\x07":
            zip64 = struct.unpack("<Q", data[end - 12:end - 4])[0]
            count = struct.unpack("<Q", data[zip64 + 32:zip64 + 40])[0]
            offset = struct.unpack("<Q", data[zip64 + 48:zip64 + 56])[0]
        pos = offset
        for _ in range(count):
            if data[pos:pos + 4] != b"PK\x01\x02":
                raise BadZipFile("Bad magic number for central directory")
            (flags, compress, dostime, dosdate, crc, csize, usize, nlen, elen, clen, ext_attr, hoff) = (
                struct.unpack("<H", data[pos + 8:pos + 10])[0],
                struct.unpack("<H", data[pos + 10:pos + 12])[0],
                struct.unpack("<H", data[pos + 12:pos + 14])[0],
                struct.unpack("<H", data[pos + 14:pos + 16])[0],
                struct.unpack("<I", data[pos + 16:pos + 20])[0],
                struct.unpack("<I", data[pos + 20:pos + 24])[0],
                struct.unpack("<I", data[pos + 24:pos + 28])[0],
                struct.unpack("<H", data[pos + 28:pos + 30])[0],
                struct.unpack("<H", data[pos + 30:pos + 32])[0],
                struct.unpack("<H", data[pos + 32:pos + 34])[0],
                struct.unpack("<I", data[pos + 38:pos + 42])[0],
                struct.unpack("<I", data[pos + 42:pos + 46])[0])
            name = data[pos + 46:pos + 46 + nlen].decode("utf-8")
            extra = data[pos + 46 + nlen:pos + 46 + nlen + elen]
            if csize == 0xFFFFFFFF or usize == 0xFFFFFFFF or hoff == 0xFFFFFFFF:
                usize, csize, hoff = _zip64(extra, usize, csize, hoff)
            dt = _decode_dos_datetime(dostime, dosdate)
            if dt[0] < 1980 or dt[1] == 0 or dt[2] == 0:
                dt = (1980, 1, 1, 0, 0, 0)
            info = ZipInfo(name, dt)
            info.flag_bits = flags
            info.compress_type = compress
            info.CRC = crc
            info.compress_size = csize
            info.file_size = usize
            info.external_attr = ext_attr
            info.header_offset = hoff
            info._central = data[pos:pos + 46 + nlen + elen + clen]
            self._entries.append(info)
            pos += 46 + nlen + elen + clen
        return offset

    def namelist(self):
        return [e.filename for e in self._entries]

    def infolist(self):
        return list(self._entries)

    def getinfo(self, name):
        for e in self._entries:
            if e.filename == name:
                return e
        raise KeyError("There is no item named %r in the archive" % name)

    def read(self, name, pwd=None):
        info = self.getinfo(name) if not isinstance(name, ZipInfo) else name
        data = self._data
        pos = info.header_offset
        if data[pos:pos + 4] != b"PK\x03\x04":
            raise BadZipFile("Bad magic number for file header")
        nlen = struct.unpack("<H", data[pos + 26:pos + 28])[0]
        elen = struct.unpack("<H", data[pos + 28:pos + 30])[0]
        start = pos + 30 + nlen + elen
        if info.compress_type == ZIP_STORED:
            raw = data[start:start + info.file_size]
        elif info.compress_type == ZIP_DEFLATED:
            raw = zlib.decompress(data[start:start + info.compress_size], -15)
        else:
            raise NotImplementedError("compression method %d is not supported" % info.compress_type)
        if _crc32(raw) != info.CRC:
            raise BadZipFile("Bad CRC-32 for file %r" % info.filename)
        return raw

    def open(self, name, mode="r", pwd=None, *, force_zip64=False):
        from io import BytesIO
        if mode != "r":
            raise NotImplementedError("ZipFile.open() only supports mode 'r'; use writestr() or write()")
        return BytesIO(self.read(name, pwd))

    def extract(self, member, path=None, pwd=None):
        info = self.getinfo(member) if not isinstance(member, ZipInfo) else member
        base = "" if path is None else (path.__fspath__() if hasattr(path, "__fspath__") else str(path))
        parts = [p for p in info.filename.replace("\\", "/").split("/") if p and p != "."]
        if any(p == ".." for p in parts):
            raise BadZipFile("Unsafe archive member path %r" % info.filename)
        target = os.path.join(base, *parts) if base else os.path.join(*parts) if parts else ""
        if info.is_dir():
            if target:
                os.makedirs(target, exist_ok=True)
            return target
        parent = os.path.dirname(target)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(target, "wb") as f:
            f.write(self.read(info, pwd))
        return target

    def extractall(self, path=None, members=None, pwd=None):
        items = self._entries if members is None else members
        for member in items:
            self.extract(member, path, pwd)

    def writestr(self, zinfo_or_arcname, data, compress_type=None, compresslevel=None):
        if self.mode == "r":
            raise ValueError("write() requires mode 'w', 'x', or 'a'")
        if self._closed:
            raise ValueError("Attempt to write to ZIP archive that was already closed")
        if isinstance(zinfo_or_arcname, ZipInfo):
            zinfo = zinfo_or_arcname
            if compress_type is not None:
                zinfo.compress_type = compress_type
            level = compresslevel if compresslevel is not None else (
                zinfo._compresslevel if zinfo._compresslevel is not None else self.compresslevel
            )
        else:
            zinfo = ZipInfo(filename=str(zinfo_or_arcname))
            zinfo.compress_type = self.compression if compress_type is None else compress_type
            level = compresslevel if compresslevel is not None else self.compresslevel
        _check_compression(zinfo.compress_type)
        if isinstance(data, str):
            raw = data.encode("utf-8")
        elif isinstance(data, bytes):
            raw = data
        else:
            raw = bytes(data)
        crc = _crc32(raw)
        if zinfo.compress_type == ZIP_STORED:
            payload = raw
        elif zinfo.compress_type == ZIP_DEFLATED:
            lvl = -1 if level is None else int(level)
            if lvl != -1 and (lvl < 0 or lvl > 9):
                raise ValueError("Bad compression level")
            payload = zlib.compress(raw, lvl, -15)
        else:
            raise NotImplementedError("That compression method is not supported")
        zinfo.CRC = crc
        zinfo.file_size = len(raw)
        zinfo.compress_size = len(payload)
        self._records.append((zinfo, payload))
        self._entries.append(zinfo)

    def write(self, filename, arcname=None, compress_type=None, compresslevel=None):
        if hasattr(filename, "__fspath__"):
            filename = filename.__fspath__()
        with open(filename, "rb") as f:
            self.writestr(arcname or filename, f.read(), compress_type=compress_type, compresslevel=compresslevel)

    def close(self):
        if self._closed:
            return
        self._closed = True
        if self.mode == "r":
            return
        out = [self._prefix]
        central = [info._central for info in self._entries if getattr(info, "_central", None) is not None]
        offset = len(self._prefix)
        for zinfo, payload in self._records:
            enc = zinfo.filename.encode("utf-8")
            extra = zinfo.extra if isinstance(zinfo.extra, bytes) else bytes(zinfo.extra)
            comment = zinfo.comment if isinstance(zinfo.comment, bytes) else bytes(zinfo.comment)
            flags = zinfo.flag_bits
            if any(b >= 128 for b in enc):
                flags |= 0x800
            dostime, dosdate = _encode_dos_datetime(zinfo.date_time)
            zinfo.header_offset = offset
            header = (
                b"PK\x03\x04"
                + struct.pack(
                    "<HHHHHIIIHH",
                    zinfo.extract_version,
                    flags,
                    zinfo.compress_type,
                    dostime,
                    dosdate,
                    zinfo.CRC,
                    zinfo.compress_size,
                    zinfo.file_size,
                    len(enc),
                    len(extra),
                )
                + enc
                + extra
            )
            out.append(header)
            out.append(payload)
            central.append(
                b"PK\x01\x02"
                + struct.pack(
                    "<HHHHHHIIIHHHHHII",
                    (zinfo.create_system << 8) | zinfo.create_version,
                    zinfo.extract_version,
                    flags,
                    zinfo.compress_type,
                    dostime,
                    dosdate,
                    zinfo.CRC,
                    zinfo.compress_size,
                    zinfo.file_size,
                    len(enc),
                    len(extra),
                    len(comment),
                    zinfo.volume,
                    zinfo.internal_attr,
                    zinfo.external_attr,
                    offset,
                )
                + enc
                + extra
                + comment
            )
            offset += len(header) + len(payload)
        cd = b"".join(central)
        end = b"PK\x05\x06" + struct.pack("<HHHHIIH", 0, 0, len(central), len(central), len(cd), offset, 0)
        out.append(cd)
        out.append(end)
        blob = _concat(out)
        if self._file is not None:
            self._file.write(blob)
        else:
            with open(self.filename, "wb") as f:
                f.write(blob)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False

    def testzip(self):
        for info in self._entries:
            try:
                self.read(info)
            except Exception:
                return info.filename
        return None


def _zip64(extra, usize, csize, hoff):
    pos = 0
    while pos + 4 <= len(extra):
        tag = struct.unpack("<H", extra[pos:pos + 2])[0]
        size = struct.unpack("<H", extra[pos + 2:pos + 4])[0]
        if tag == 1:
            fields = []
            p = pos + 4
            while p + 8 <= pos + 4 + size:
                fields.append(struct.unpack("<Q", extra[p:p + 8])[0])
                p += 8
            values = list(fields)
            if usize == 0xFFFFFFFF and values:
                usize = values.pop(0)
            if csize == 0xFFFFFFFF and values:
                csize = values.pop(0)
            if hoff == 0xFFFFFFFF and values:
                hoff = values.pop(0)
            return usize, csize, hoff
        pos += 4 + size
    return usize, csize, hoff


def is_zipfile(file):
    try:
        ZipFile(file)
        return True
    except Exception:
        return False
