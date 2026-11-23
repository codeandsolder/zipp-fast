"""RFC 3986 URL parsing, joining, quoting and query-string helpers (`urllib.parse`)."""
import re

__all__ = [
    "urlparse", "urlunparse", "urljoin", "urldefrag", "urlsplit", "urlunsplit",
    "urlencode", "parse_qs", "parse_qsl", "quote", "quote_plus",
    "quote_from_bytes", "unquote", "unquote_plus", "unquote_to_bytes",
    "ParseResult", "SplitResult", "DefragResult", "uses_relative",
    "uses_netloc", "uses_params", "uses_query", "uses_fragment", "scheme_chars",
]

uses_relative = [
    "", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https",
    "shttp", "mms", "prospero", "rtsp", "rtsps", "rtspu", "sftp", "svn",
    "svn+ssh", "ws", "wss",
]
uses_netloc = [
    "", "ftp", "http", "gopher", "nntp", "telnet", "imap", "wais", "file",
    "mms", "https", "shttp", "snews", "prospero", "rtsp", "rtsps", "rtspu",
    "rsync", "svn", "svn+ssh", "sftp", "nfs", "git", "git+ssh", "ws", "wss",
    "itms-services",
]
uses_params = [
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp",
    "rtsps", "rtspu", "sip", "sips", "mms", "sftp", "tel",
]
uses_query = [
    "", "http", "wais", "imap", "https", "shttp", "mms", "gopher", "rtsp",
    "rtsps", "rtspu", "sip", "sips",
]
uses_fragment = [
    "", "ftp", "hdl", "http", "gopher", "news", "nntp", "wais", "https",
    "shttp", "snews", "file", "prospero",
]

scheme_chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-."
_UNSAFE_URL_BYTES_TO_REMOVE = ["\t", "\r", "\n"]
_ALWAYS_SAFE = frozenset(
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-~"
)
_HEXDIG = "0123456789ABCDEF"


class _NetlocResultMixin:
    @property
    def username(self):
        netloc = self.netloc
        if "@" not in netloc:
            return None
        userinfo = netloc.rsplit("@", 1)[0]
        if ":" in userinfo:
            return userinfo.split(":", 1)[0]
        return userinfo

    @property
    def password(self):
        netloc = self.netloc
        if "@" not in netloc:
            return None
        userinfo = netloc.rsplit("@", 1)[0]
        if ":" in userinfo:
            return userinfo.split(":", 1)[1]
        return None

    @property
    def hostname(self):
        netloc = self.netloc
        if "@" in netloc:
            netloc = netloc.rsplit("@", 1)[1]
        if "[" in netloc and "]" in netloc:
            host = netloc.split("[", 1)[1].split("]", 1)[0]
            return host.lower() or None
        if ":" in netloc:
            netloc = netloc.split(":", 1)[0]
        return netloc.lower() or None

    @property
    def port(self):
        netloc = self.netloc
        if "@" in netloc:
            netloc = netloc.rsplit("@", 1)[1]
        if "]" in netloc:
            netloc = netloc.split("]", 1)[1]
        if ":" not in netloc:
            return None
        port_s = netloc.split(":", 1)[1]
        if not port_s:
            return None
        port = int(port_s, 10)
        if not (0 <= port <= 65535):
            raise ValueError("Port out of range 0-65535")
        return port


class SplitResult(tuple, _NetlocResultMixin):
    _fields = ("scheme", "netloc", "path", "query", "fragment")

    def __new__(cls, scheme, netloc, path, query, fragment):
        return tuple.__new__(cls, (scheme, netloc, path, query, fragment))

    @property
    def scheme(self):
        return self[0]

    @property
    def netloc(self):
        return self[1]

    @property
    def path(self):
        return self[2]

    @property
    def query(self):
        return self[3]

    @property
    def fragment(self):
        return self[4]

    def geturl(self):
        return urlunsplit(self)

    def _replace(self, **kw):
        return SplitResult(
            kw.get("scheme", self[0]),
            kw.get("netloc", self[1]),
            kw.get("path", self[2]),
            kw.get("query", self[3]),
            kw.get("fragment", self[4]),
        )

    def __repr__(self):
        return "SplitResult(scheme=%r, netloc=%r, path=%r, query=%r, fragment=%r)" % self


class ParseResult(tuple, _NetlocResultMixin):
    _fields = ("scheme", "netloc", "path", "params", "query", "fragment")

    def __new__(cls, scheme, netloc, path, params, query, fragment):
        return tuple.__new__(cls, (scheme, netloc, path, params, query, fragment))

    @property
    def scheme(self):
        return self[0]

    @property
    def netloc(self):
        return self[1]

    @property
    def path(self):
        return self[2]

    @property
    def params(self):
        return self[3]

    @property
    def query(self):
        return self[4]

    @property
    def fragment(self):
        return self[5]

    def geturl(self):
        return urlunparse(self)

    def _replace(self, **kw):
        return ParseResult(
            kw.get("scheme", self[0]),
            kw.get("netloc", self[1]),
            kw.get("path", self[2]),
            kw.get("params", self[3]),
            kw.get("query", self[4]),
            kw.get("fragment", self[5]),
        )

    def __repr__(self):
        return "ParseResult(scheme=%r, netloc=%r, path=%r, params=%r, query=%r, fragment=%r)" % self


class DefragResult(tuple):
    _fields = ("url", "fragment")

    def __new__(cls, url, fragment):
        return tuple.__new__(cls, (url, fragment))

    @property
    def url(self):
        return self[0]

    @property
    def fragment(self):
        return self[1]

    def geturl(self):
        if self[1]:
            return self[0] + "#" + self[1]
        return self[0]


def _remove_unsafe_bytes_from_url(url):
    for b in _UNSAFE_URL_BYTES_TO_REMOVE:
        url = url.replace(b, "")
    return url


def _splitnetloc(url, start=0):
    delim = len(url)
    for c in "/?#":
        wdelim = url.find(c, start)
        if wdelim >= 0:
            delim = min(delim, wdelim)
    return url[start:delim], url[delim:]


def _splitparams(url):
    if "/" in url:
        i = url.find(";", url.rfind("/"))
        if i < 0:
            return url, ""
    else:
        i = url.find(";")
        if i < 0:
            return url, ""
    return url[:i], url[i + 1:]


def urlsplit(url, scheme="", allow_fragments=True):
    url = _remove_unsafe_bytes_from_url(url)
    scheme = _remove_unsafe_bytes_from_url(scheme)
    allow_fragments = bool(allow_fragments)
    netloc = query = fragment = ""
    i = url.find(":")
    if i > 0 and url[0].isascii() and url[0].isalpha():
        for c in url[:i]:
            if c not in scheme_chars:
                break
        else:
            scheme, url = url[:i].lower(), url[i + 1:]
    if url[:2] == "//":
        netloc, url = _splitnetloc(url, 2)
    if allow_fragments and "#" in url:
        url, fragment = url.split("#", 1)
    if "?" in url:
        url, query = url.split("?", 1)
    return SplitResult(scheme, netloc, url, query, fragment)


def urlparse(url, scheme="", allow_fragments=True):
    scheme, netloc, url, query, fragment = urlsplit(url, scheme, allow_fragments)
    if scheme in uses_params and ";" in url:
        url, params = _splitparams(url)
    else:
        params = ""
    return ParseResult(scheme, netloc, url, params, query, fragment)


def urlunsplit(components):
    scheme, netloc, url, query, fragment = components
    if netloc or (scheme and scheme in uses_netloc and url[:2] != "//"):
        if url and url[:1] != "/":
            url = "/" + url
        url = "//" + (netloc or "") + url
    if scheme:
        url = scheme + ":" + url
    if query:
        url = url + "?" + query
    if fragment:
        url = url + "#" + fragment
    return url


def urlunparse(components):
    scheme, netloc, url, params, query, fragment = components
    if params:
        url = "%s;%s" % (url, params)
    return urlunsplit((scheme, netloc, url, query, fragment))


def urljoin(base, url, allow_fragments=True):
    if not base:
        return url
    if not url:
        return base
    bscheme, bnetloc, bpath, bparams, bquery, bfragment = urlparse(base, "", allow_fragments)
    scheme, netloc, path, params, query, fragment = urlparse(url, bscheme, allow_fragments)
    if scheme != bscheme or scheme not in uses_relative:
        return url
    if scheme in uses_netloc:
        if netloc:
            return urlunparse((scheme, netloc, path, params, query, fragment))
        netloc = bnetloc
    if not path and not params:
        path = bpath
        params = bparams
        if not query:
            query = bquery
        return urlunparse((scheme, netloc, path, params, query, fragment))
    base_parts = bpath.split("/")
    if base_parts[-1] != "":
        del base_parts[-1]
    if path[:1] == "/":
        segments = path.split("/")
    else:
        segments = base_parts + path.split("/")
        segments[1:-1] = [s for s in segments[1:-1] if s]
    resolved_path = []
    for seg in segments:
        if seg == "..":
            try:
                resolved_path.pop()
            except IndexError:
                pass
        elif seg == ".":
            continue
        else:
            resolved_path.append(seg)
    if segments[-1] in (".", ".."):
        resolved_path.append("")
    return urlunparse((scheme, netloc, "/".join(resolved_path) or "/", params, query, fragment))


def urldefrag(url):
    if "#" in url:
        s, n, p, a, q, frag = urlparse(url)
        defrag = urlunparse((s, n, p, a, q, ""))
    else:
        frag = ""
        defrag = url
    return DefragResult(defrag, frag)


def quote_from_bytes(bs, safe="/"):
    if not isinstance(bs, (bytes, bytearray)):
        raise TypeError("quote_from_bytes() expected bytes")
    if not bs:
        return ""
    if isinstance(safe, str):
        safe = safe.encode("ascii", "ignore")
    safe_set = _ALWAYS_SAFE | frozenset(safe)
    out = []
    for b in bs:
        if b in safe_set:
            out.append(chr(b))
        else:
            out.append("%" + _HEXDIG[b >> 4] + _HEXDIG[b & 0xF])
    return "".join(out)


def quote(string, safe="/", encoding=None, errors=None):
    if isinstance(string, str):
        if not string:
            return string
        if encoding is None:
            encoding = "utf-8"
        if errors is None:
            errors = "strict"
        string = string.encode(encoding, errors)
    else:
        if encoding is not None:
            raise TypeError("quote() doesn't support 'encoding' for bytes")
        if errors is not None:
            raise TypeError("quote() doesn't support 'errors' for bytes")
    return quote_from_bytes(string, safe)


def quote_plus(string, safe="", encoding=None, errors=None):
    if (isinstance(string, str) and " " not in string) or (
        isinstance(string, (bytes, bytearray)) and b" " not in string
    ):
        return quote(string, safe, encoding, errors)
    if isinstance(safe, str):
        space = " "
    else:
        space = b" "
    string = quote(string, safe + space, encoding, errors)
    return string.replace(" ", "+")


_HEX_RE = re.compile(r"%([0-9a-fA-F]{2})")


def unquote_to_bytes(string):
    if not string:
        return b""
    if isinstance(string, str):
        string = string.encode("utf-8")
    out = bytearray()
    i = 0
    n = len(string)
    while i < n:
        b = string[i]
        if b == 37 and i + 2 < n:
            h1, h2 = string[i + 1], string[i + 2]
            v1 = _hexval(h1)
            v2 = _hexval(h2)
            if v1 >= 0 and v2 >= 0:
                out.append((v1 << 4) | v2)
                i += 3
                continue
        out.append(b)
        i += 1
    return bytes(out)


def _hexval(b):
    if 48 <= b <= 57:
        return b - 48
    if 65 <= b <= 70:
        return b - 55
    if 97 <= b <= 102:
        return b - 87
    return -1


def unquote(string, encoding="utf-8", errors="replace"):
    if isinstance(string, bytes):
        return unquote_to_bytes(string).decode(encoding, errors)
    if "%" not in string:
        return string
    if encoding is None:
        encoding = "utf-8"
    if errors is None:
        errors = "replace"
    bits = string.split("%")
    res = [bits[0]]
    i = 1
    n = len(bits)
    while i < n:
        buf = bytearray()
        while i < n and len(bits[i]) >= 2:
            h1, h2 = ord(bits[i][0]), ord(bits[i][1])
            v1, v2 = _hexval(h1), _hexval(h2)
            if v1 < 0 or v2 < 0:
                break
            buf.append((v1 << 4) | v2)
            rest = bits[i][2:]
            i += 1
            if rest:
                res.append(bytes(buf).decode(encoding, errors))
                res.append(rest)
                buf = bytearray()
                break
        if buf:
            res.append(bytes(buf).decode(encoding, errors))
        elif i < n:
            res.append("%" + bits[i])
            i += 1
    return "".join(res)


def unquote_plus(string, encoding="utf-8", errors="replace"):
    return unquote(string.replace("+", " "), encoding, errors)


def parse_qsl(qs, keep_blank_values=False, strict_parsing=False, encoding="utf-8", errors="replace", max_num_fields=None, separator="&"):
    if not qs:
        return []
    pairs = qs.split(separator)
    if max_num_fields is not None and len(pairs) > max_num_fields:
        raise ValueError("Max number of fields exceeded")
    r = []
    for name_value in pairs:
        if not name_value and not strict_parsing:
            continue
        nv = name_value.split("=", 1)
        if len(nv) != 2:
            if strict_parsing:
                raise ValueError("bad query field: %r" % (name_value,))
            if keep_blank_values:
                nv.append("")
            else:
                continue
        if len(nv[1]) or keep_blank_values:
            name = unquote_plus(nv[0], encoding=encoding, errors=errors)
            value = unquote_plus(nv[1], encoding=encoding, errors=errors)
            r.append((name, value))
    return r


def parse_qs(qs, keep_blank_values=False, strict_parsing=False, encoding="utf-8", errors="replace", max_num_fields=None, separator="&"):
    parsed_result = {}
    pairs = parse_qsl(qs, keep_blank_values, strict_parsing, encoding=encoding, errors=errors, max_num_fields=max_num_fields, separator=separator)
    for name, value in pairs:
        if name in parsed_result:
            parsed_result[name].append(value)
        else:
            parsed_result[name] = [value]
    return parsed_result


def urlencode(query, doseq=False, safe="", encoding=None, errors=None, quote_via=quote_plus):
    if hasattr(query, "items"):
        query = query.items()
    parts = []
    for k, v in query:
        k_enc = quote_via(str(k), safe, encoding, errors)
        if doseq and isinstance(v, (list, tuple)):
            for elem in v:
                parts.append(k_enc + "=" + quote_via(str(elem), safe, encoding, errors))
        else:
            parts.append(k_enc + "=" + quote_via(str(v), safe, encoding, errors))
    return "&".join(parts)
