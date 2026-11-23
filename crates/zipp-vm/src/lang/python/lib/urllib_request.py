"""Sandbox-safe urllib.request for Zipp Python (file/data URLs + Request/Opener types; no host network)."""
import base64
import io
import urllib.error
import urllib.parse
import urllib.response

__all__ = [
    "Request", "BaseHandler", "OpenerDirector", "build_opener",
    "install_opener", "urlopen", "urlretrieve", "pathname2url",
    "url2pathname", "getproxies",
]


def pathname2url(pathname):
    return urllib.parse.quote(pathname)


def url2pathname(pathname):
    return urllib.parse.unquote(pathname)


def getproxies():
    return {}


class Request:
    def __init__(self, url, data=None, headers=None, origin_req_host=None, unverifiable=False, method=None):
        self.full_url = url
        self.data = data
        self.headers = dict(headers) if headers else {}
        self.unredirected_hdrs = {}
        self.origin_req_host = origin_req_host
        self.unverifiable = unverifiable
        self.method = method

    @property
    def full_url(self):
        return self._full_url

    @full_url.setter
    def full_url(self, url):
        self._full_url = urllib.parse.unwrap(url) if hasattr(urllib.parse, "unwrap") else url
        self._full_url, self.fragment = urllib.parse.urldefrag(self._full_url)
        parsed = urllib.parse.urlsplit(self._full_url)
        self.type = parsed.scheme
        self.host = parsed.netloc
        self.selector = parsed.path + (("?" + parsed.query) if parsed.query else "")

    def get_method(self):
        if self.method is not None:
            return self.method
        return "POST" if self.data is not None else "GET"

    def add_header(self, key, val):
        self.headers[key.capitalize()] = val

    def get_header(self, header_name, default=None):
        return self.headers.get(header_name, self.headers.get(header_name.capitalize(), default))

    def has_header(self, header_name):
        return header_name in self.headers or header_name.capitalize() in self.headers

    def header_items(self):
        return list(self.headers.items())


class BaseHandler:
    handler_order = 500

    def add_parent(self, parent):
        self.parent = parent

    def close(self):
        pass


class OpenerDirector:
    def __init__(self):
        self.addheaders = [("User-agent", "Python-urllib/3.12")]
        self.handlers = []

    def add_handler(self, handler):
        self.handlers.append(handler)
        if hasattr(handler, "add_parent"):
            handler.add_parent(self)

    def open(self, fullurl, data=None, timeout=None):
        return urlopen(fullurl, data=data, timeout=timeout)

    def close(self):
        pass


_opener = None


def build_opener(*handlers):
    opener = OpenerDirector()
    for h in handlers:
        opener.add_handler(h() if isinstance(h, type) else h)
    return opener


def install_opener(opener):
    global _opener
    _opener = opener


def urlopen(url, data=None, timeout=None, *, cafile=None, capath=None, cadefault=False, context=None):
    target = url.full_url if isinstance(url, Request) else str(url)
    parsed = urllib.parse.urlsplit(target)
    if parsed.scheme == "data":
        rest = parsed.path
        if "," not in rest:
            raise urllib.error.URLError("malformed data URL")
        meta, payload = rest.split(",", 1)
        if meta.endswith(";base64"):
            raw = base64.b64decode(payload)
        else:
            raw = urllib.parse.unquote_to_bytes(payload)
        return urllib.response.addinfourl(io.BytesIO(raw), {}, target, 200)
    if parsed.scheme == "file":
        path = url2pathname(parsed.path).lstrip("/")
        try:
            f = open(path, "rb")
        except OSError as exc:
            raise urllib.error.URLError(exc)
        return urllib.response.addinfourl(f, {}, target, 200)
    raise urllib.error.URLError("network access is not available in the Python sandbox: %s" % target)


def urlretrieve(url, filename=None, reporthook=None, data=None):
    with urlopen(url, data=data) as resp:
        raw = resp.read()
        if filename is not None:
            with open(filename, "wb") as f:
                f.write(raw)
        return filename, resp.info()
