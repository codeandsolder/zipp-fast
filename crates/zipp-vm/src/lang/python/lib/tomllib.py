"""TOML 1.0.0 parser (`tomllib`) for Zipp."""
import datetime as _dt
import re

__all__ = ("loads", "load", "TOMLDecodeError")

_WS = " \t"
_BARE_KEY = re.compile(r"[A-Za-z0-9_-]+")
_CTRL_BAD = frozenset(range(0x00, 0x09)) | frozenset(range(0x0A, 0x20)) | {0x7F}
_CTRL_ML_BAD = frozenset(range(0x00, 0x09)) | {0x0B, 0x0C} | frozenset(range(0x0E, 0x20)) | {0x7F}

_ESC_MAP = {
    "b": "\b",
    "t": "\t",
    "n": "\n",
    "f": "\f",
    "r": "\r",
    '"': '"',
    "\\": "\\",
}

_DT_RE = re.compile(
    r"^(\d{4})-(\d{2})-(\d{2})"
    r"(?:[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(?:([Zz])|([+-])(\d{2}):(\d{2}))?)?$"
)
_TIME_RE = re.compile(r"^(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?$")
_HEX_RE = re.compile(r"^0x([0-9A-Fa-f](?:_?[0-9A-Fa-f])*)$")
_OCT_RE = re.compile(r"^0o([0-7](?:_?[0-7])*)$")
_BIN_RE = re.compile(r"^0b([01](?:_?[01])*)$")
_DEC_INT_RE = re.compile(r"^([+-])?(0|[1-9](?:_?[0-9])*)$")
_FLOAT_RE = re.compile(
    r"^[+-]?(?:0|[1-9](?:_?[0-9])*)(?:\.[0-9](?:_?[0-9])*(?:[eE][+-]?[0-9](?:_?[0-9])*)?|[eE][+-]?[0-9](?:_?[0-9])*)$"
)


class TOMLDecodeError(ValueError):
    def __init__(self, msg, doc="", pos=0):
        lineno = doc.count("\n", 0, pos) + 1 if doc else 1
        last_nl = doc.rfind("\n", 0, pos) if doc else -1
        colno = pos - last_nl if doc else 1
        self.msg = msg
        self.doc = doc
        self.pos = pos
        self.lineno = lineno
        self.colno = colno
        full = "%s (at line %d, column %d)" % (msg, lineno, colno) if doc else msg
        super().__init__(full)


def load(fp, *, parse_float=float):
    raw = fp.read()
    if not isinstance(raw, (bytes, bytearray)):
        raise TypeError("File must be opened in binary mode, e.g. use `open('foo.toml', 'rb')`")
    try:
        text = bytes(raw).decode("utf-8")
    except UnicodeDecodeError as e:
        raise TOMLDecodeError("Invalid UTF-8 in TOML document: %s" % e)
    return loads(text, parse_float=parse_float)


def loads(s, *, parse_float=float):
    if not isinstance(s, str):
        raise TypeError("Expected str object, not %r" % type(s).__name__)
    # Reject bare CR not followed by LF per TOML 1.0.0.
    i = s.find("\r")
    while i >= 0:
        if i + 1 >= len(s) or s[i + 1] != "\n":
            raise TOMLDecodeError("Bare CR is not permitted in TOML", s, i)
        i = s.find("\r", i + 2)
    src = s.replace("\r\n", "\n")
    parser = _TomlParser(src, parse_float)
    return parser.parse_document()


class _TomlParser:
    def __init__(self, src, parse_float):
        self.src = src
        self.n = len(src)
        self.pos = 0
        self.parse_float = parse_float
        self.root = {}
        # Track explicit headers, implicit dotted-key tables, static arrays/inline tables.
        self._explicit = set()
        self._dotted = set()
        self._frozen = set()
        self._aot = set()

    def err(self, msg, pos=None):
        raise TOMLDecodeError(msg, self.src, self.pos if pos is None else pos)

    def _skip_ws(self):
        s, n, p = self.src, self.n, self.pos
        while p < n and s[p] in _WS:
            p += 1
        self.pos = p

    def _skip_comment(self):
        s, n, p = self.src, self.n, self.pos
        if p < n and s[p] == "#":
            p += 1
            while p < n and s[p] != "\n":
                if ord(s[p]) in _CTRL_BAD:
                    self.err("Invalid control character in comment", p)
                p += 1
            self.pos = p

    def _skip_ws_nl_comments(self):
        s, n = self.src, self.n
        while self.pos < n:
            c = s[self.pos]
            if c in " \t\n":
                self.pos += 1
            elif c == "#":
                self._skip_comment()
            else:
                break

    def _expect_eol(self):
        self._skip_ws()
        self._skip_comment()
        if self.pos < self.n:
            if self.src[self.pos] == "\n":
                self.pos += 1
            else:
                self.err("Expected newline or end of file")

    def parse_document(self):
        cur_table = self.root
        cur_path = ()
        while True:
            self._skip_ws_nl_comments()
            if self.pos >= self.n:
                break
            c = self.src[self.pos]
            if c == "[":
                if self.pos + 1 < self.n and self.src[self.pos + 1] == "[":
                    cur_table, cur_path = self._parse_aot_header()
                else:
                    cur_table, cur_path = self._parse_table_header()
                self._expect_eol()
            else:
                self._parse_keyval(cur_table, cur_path, inline=False)
                self._expect_eol()
        return self.root

    def _parse_table_header(self):
        start = self.pos
        self.pos += 1
        self._skip_ws()
        keys = self._parse_dotted_key()
        self._skip_ws()
        if self.pos >= self.n or self.src[self.pos] != "]":
            self.err("Expected ']' to close table header")
        self.pos += 1
        table, full_path = self._navigate_header(keys, is_aot=False, err_pos=start)
        return table, full_path

    def _parse_aot_header(self):
        start = self.pos
        self.pos += 2
        self._skip_ws()
        keys = self._parse_dotted_key()
        self._skip_ws()
        if self.pos + 1 >= self.n or self.src[self.pos:self.pos + 2] != "]]":
            self.err("Expected ']]' to close array-of-tables header")
        self.pos += 2
        table, full_path = self._navigate_header(keys, is_aot=True, err_pos=start)
        return table, full_path

    def _navigate_header(self, keys, is_aot, err_pos):
        node = self.root
        path = ()
        for i, k in enumerate(keys):
            is_last = i == len(keys) - 1
            if id(node) in self._frozen:
                self.err("Cannot extend immutable inline table", err_pos)
            nxt_path = path + (k,)
            if is_last:
                if is_aot:
                    if k not in node:
                        arr = []
                        node[k] = arr
                        self._aot.add(id(arr))
                    else:
                        arr = node[k]
                        if not isinstance(arr, list) or id(arr) not in self._aot:
                            self.err("Cannot redefine %r as array of tables" % k, err_pos)
                    fresh = {}
                    arr.append(fresh)
                    return fresh, nxt_path + (len(arr) - 1,)
                else:
                    if k in node:
                        existing = node[k]
                        if not isinstance(existing, dict):
                            self.err("Cannot overwrite non-table key %r" % k, err_pos)
                        if id(existing) in self._explicit or id(existing) in self._frozen or id(existing) in self._dotted:
                            self.err("Duplicate table header %r" % (".".join(keys),), err_pos)
                        self._explicit.add(id(existing))
                        return existing, nxt_path
                    fresh = {}
                    node[k] = fresh
                    self._explicit.add(id(fresh))
                    return fresh, nxt_path
            else:
                if k not in node:
                    sub = {}
                    node[k] = sub
                    node = sub
                    path = nxt_path
                else:
                    existing = node[k]
                    if isinstance(existing, dict):
                        if id(existing) in self._frozen:
                            self.err("Cannot extend inline table %r" % k, err_pos)
                        node = existing
                        path = nxt_path
                    elif isinstance(existing, list) and id(existing) in self._aot:
                        node = existing[-1]
                        path = nxt_path + (len(existing) - 1,)
                    else:
                        self.err("Cannot traverse non-table key %r" % k, err_pos)
        return node, path

    def _parse_keyval(self, target, path, inline):
        start = self.pos
        keys = self._parse_dotted_key()
        self._skip_ws()
        if self.pos >= self.n or self.src[self.pos] != "=":
            self.err("Expected '=' after key")
        self.pos += 1
        self._skip_ws()
        val = self._parse_value()
        node = target
        for k in keys[:-1]:
            if id(node) in self._frozen:
                self.err("Cannot mutate inline table via dotted key", start)
            if k not in node:
                sub = {}
                node[k] = sub
                self._dotted.add(id(sub))
                node = sub
            else:
                existing = node[k]
                if not isinstance(existing, dict) or id(existing) in self._frozen:
                    self.err("Cannot redefine key %r as table" % k, start)
                if not inline and id(existing) in self._explicit:
                    self.err("Cannot extend previously defined table %r with dotted key" % k, start)
                node = existing
        leaf = keys[-1]
        if leaf in node:
            self.err("Duplicate key %r" % leaf, start)
        node[leaf] = val

    def _parse_dotted_key(self):
        keys = [self._parse_single_key()]
        while True:
            self._skip_ws()
            if self.pos < self.n and self.src[self.pos] == ".":
                self.pos += 1
                self._skip_ws()
                keys.append(self._parse_single_key())
            else:
                break
        return keys

    def _parse_single_key(self):
        if self.pos >= self.n:
            self.err("Expected key")
        c = self.src[self.pos]
        if c == '"':
            if self.src[self.pos:self.pos + 3] == '"""':
                self.err("Multiline string cannot be used as key")
            return self._parse_basic_string()
        if c == "'":
            if self.src[self.pos:self.pos + 3] == "'''":
                self.err("Multiline literal string cannot be used as key")
            return self._parse_literal_string()
        m = _BARE_KEY.match(self.src, self.pos)
        if not m:
            self.err("Invalid key")
        self.pos = m.end()
        return m.group(0)

    def _parse_value(self):
        if self.pos >= self.n:
            self.err("Expected value")
        c = self.src[self.pos]
        if c == '"':
            if self.src[self.pos:self.pos + 3] == '"""':
                return self._parse_multiline_basic_string()
            return self._parse_basic_string()
        if c == "'":
            if self.src[self.pos:self.pos + 3] == "'''":
                return self._parse_multiline_literal_string()
            return self._parse_literal_string()
        if c == "[":
            return self._parse_array()
        if c == "{":
            return self._parse_inline_table()
        return self._parse_scalar()

    def _parse_basic_string(self):
        start = self.pos
        self.pos += 1
        out = []
        s, n = self.src, self.n
        while self.pos < n:
            c = s[self.pos]
            if c == '"':
                self.pos += 1
                return "".join(out)
            if c == "\n":
                self.err("Unterminated basic string", start)
            if c == "\\":
                self.pos += 1
                out.append(self._parse_escape())
            else:
                if ord(c) in _CTRL_BAD:
                    self.err("Invalid control character in string")
                out.append(c)
                self.pos += 1
        self.err("Unterminated basic string", start)

    def _parse_multiline_basic_string(self):
        start = self.pos
        self.pos += 3
        s, n = self.src, self.n
        if self.pos < n and s[self.pos] == "\n":
            self.pos += 1
        out = []
        while self.pos < n:
            c = s[self.pos]
            if c == '"' and s[self.pos:self.pos + 3] == '"""':
                qend = self.pos
                while qend < n and s[qend] == '"':
                    qend += 1
                qcount = qend - self.pos
                if qcount <= 5:
                    out.append('"' * (qcount - 3))
                    self.pos = qend
                    return "".join(out)
                self.err("Too many consecutive quotes in multiline string")
            if c == "\\":
                self.pos += 1
                if self.pos < n and s[self.pos] in " \t\n":
                    p = self.pos
                    while p < n and s[p] in " \t":
                        p += 1
                    if p < n and s[p] == "\n":
                        while p < n and s[p] in " \t\n":
                            p += 1
                        self.pos = p
                        continue
                    self.err("Invalid line-ending backslash escape")
                out.append(self._parse_escape())
            else:
                if ord(c) in _CTRL_ML_BAD:
                    self.err("Invalid control character in multiline string")
                out.append(c)
                self.pos += 1
        self.err("Unterminated multiline basic string", start)

    def _parse_escape(self):
        if self.pos >= self.n:
            self.err("Unterminated escape sequence")
        c = self.src[self.pos]
        self.pos += 1
        if c in _ESC_MAP:
            return _ESC_MAP[c]
        if c == "u" or c == "U":
            width = 4 if c == "u" else 8
            hex_s = self.src[self.pos:self.pos + width]
            if len(hex_s) != width or not all(ch in "0123456789abcdefABCDEF" for ch in hex_s):
                self.err("Invalid Unicode escape sequence")
            self.pos += width
            cp = int(hex_s, 16)
            if 0xD800 <= cp <= 0xDFFF or cp > 0x10FFFF:
                self.err("Invalid Unicode scalar value in escape")
            return chr(cp)
        self.err("Invalid escape character %r" % c)

    def _parse_literal_string(self):
        start = self.pos
        self.pos += 1
        s, n = self.src, self.n
        begin = self.pos
        while self.pos < n:
            c = s[self.pos]
            if c == "'":
                res = s[begin:self.pos]
                self.pos += 1
                return res
            if c == "\n" or ord(c) in _CTRL_BAD:
                self.err("Invalid character in literal string")
            self.pos += 1
        self.err("Unterminated literal string", start)

    def _parse_multiline_literal_string(self):
        start = self.pos
        self.pos += 3
        s, n = self.src, self.n
        if self.pos < n and s[self.pos] == "\n":
            self.pos += 1
        out = []
        while self.pos < n:
            c = s[self.pos]
            if c == "'" and s[self.pos:self.pos + 3] == "'''":
                qend = self.pos
                while qend < n and s[qend] == "'":
                    qend += 1
                qcount = qend - self.pos
                if qcount <= 5:
                    out.append("'" * (qcount - 3))
                    self.pos = qend
                    return "".join(out)
                self.err("Too many consecutive quotes in multiline literal string")
            if ord(c) in _CTRL_ML_BAD:
                self.err("Invalid control character in multiline literal string")
            out.append(c)
            self.pos += 1
        self.err("Unterminated multiline literal string", start)

    def _parse_array(self):
        self.pos += 1
        items = []
        while True:
            self._skip_ws_nl_comments()
            if self.pos >= self.n:
                self.err("Unterminated array")
            if self.src[self.pos] == "]":
                self.pos += 1
                return items
            items.append(self._parse_value())
            self._skip_ws_nl_comments()
            if self.pos < self.n and self.src[self.pos] == ",":
                self.pos += 1
                continue
            if self.pos < self.n and self.src[self.pos] == "]":
                self.pos += 1
                return items
            self.err("Expected ',' or ']' in array")

    def _parse_inline_table(self):
        self.pos += 1
        tbl = {}
        self._skip_ws()
        if self.pos < self.n and self.src[self.pos] == "}":
            self.pos += 1
            self._frozen.add(id(tbl))
            return tbl
        while True:
            self._skip_ws()
            if self.pos >= self.n or self.src[self.pos] == "\n":
                self.err("Newline not allowed inside inline table")
            self._parse_keyval(tbl, (), inline=True)
            self._skip_ws()
            if self.pos < self.n and self.src[self.pos] == ",":
                self.pos += 1
                self._skip_ws()
                if self.pos < self.n and self.src[self.pos] == "}":
                    self.err("Trailing comma not permitted in inline table")
                continue
            if self.pos < self.n and self.src[self.pos] == "}":
                self.pos += 1
                self._frozen.add(id(tbl))
                return tbl
            self.err("Expected ',' or '}' in inline table")

    def _parse_scalar(self):
        start = self.pos
        s, n = self.src, self.n
        # Allow a single space inside RFC 3339 date-time (YYYY-MM-DD HH:MM:SS).
        p = start
        while p < n and s[p] not in " \t\n,#]}":
            p += 1
        raw = s[start:p]
        if (
            len(raw) == 10
            and raw[4] == "-"
            and raw[7] == "-"
            and p + 1 < n
            and s[p] == " "
            and s[p + 1].isdigit()
        ):
            p2 = p + 1
            while p2 < n and s[p2] not in " \t\n,#]}":
                p2 += 1
            raw = s[start:p2]
            p = p2
        self.pos = p
        if not raw:
            self.err("Expected value", start)
        if raw == "true":
            return True
        if raw == "false":
            return False
        if raw in ("inf", "+inf", "-inf", "nan", "+nan", "-nan"):
            return self.parse_float(raw)
        # Date / DateTime / Time
        if len(raw) >= 8 and (raw[4:5] == "-" or raw[2:3] == ":"):
            m_dt = _DT_RE.match(raw)
            if m_dt:
                return self._build_datetime(m_dt, start)
            m_t = _TIME_RE.match(raw)
            if m_t:
                return self._build_time(m_t, start)
        # Integers
        m = _HEX_RE.match(raw)
        if m:
            return int(m.group(1).replace("_", ""), 16)
        m = _OCT_RE.match(raw)
        if m:
            return int(m.group(1).replace("_", ""), 8)
        m = _BIN_RE.match(raw)
        if m:
            return int(m.group(1).replace("_", ""), 2)
        m = _DEC_INT_RE.match(raw)
        if m:
            return int(raw.replace("_", ""), 10)
        # Floats
        if _FLOAT_RE.match(raw):
            return self.parse_float(raw.replace("_", ""))
        self.err("Invalid value %r" % raw, start)

    def _build_datetime(self, m, pos):
        y, mo, d = int(m.group(1)), int(m.group(2)), int(m.group(3))
        if m.group(4) is None:
            try:
                return _dt.date(y, mo, d)
            except ValueError as e:
                self.err(str(e), pos)
        hh, mi, ss = int(m.group(4)), int(m.group(5)), int(m.group(6))
        frac = m.group(7) or ""
        us = int(frac[:6].ljust(6, "0")) if frac else 0
        tz = None
        if m.group(8) is not None:
            tz = _dt.timezone.utc
        elif m.group(9) is not None:
            sign = 1 if m.group(9) == "+" else -1
            oh, om = int(m.group(10)), int(m.group(11))
            if oh >= 24 or om >= 60:
                self.err("Invalid timezone offset", pos)
            delta = _dt.timedelta(hours=oh, minutes=om)
            tz = _dt.timezone(delta if sign > 0 else -delta)
        try:
            return _dt.datetime(y, mo, d, hh, mi, ss, us, tz)
        except ValueError as e:
            self.err(str(e), pos)

    def _build_time(self, m, pos):
        hh, mi, ss = int(m.group(1)), int(m.group(2)), int(m.group(3))
        frac = m.group(4) or ""
        us = int(frac[:6].ljust(6, "0")) if frac else 0
        try:
            return _dt.time(hh, mi, ss, us)
        except ValueError as e:
            self.err(str(e), pos)
