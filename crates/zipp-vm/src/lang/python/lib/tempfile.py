"""Temporary files and directories over Zipp's virtual filesystem (`tempfile`)."""
import io
import os
import shutil

_counter = 0


def _next_name(prefix="tmp", suffix=""):
    global _counter
    _counter += 1
    return "%s%06d%s" % (prefix, _counter, suffix)


def gettempdir():
    os.makedirs(".tmp", exist_ok=True)
    return ".tmp"


def mkdtemp(suffix=None, prefix=None, dir=None):
    base = gettempdir() if dir is None else str(dir)
    name = _next_name("tmp" if prefix is None else prefix, "" if suffix is None else suffix)
    path = os.path.join(base, name)
    os.makedirs(path, exist_ok=True)
    return path


class TemporaryDirectory:
    def __init__(self, suffix=None, prefix=None, dir=None, ignore_cleanup_errors=False):
        self.name = mkdtemp(suffix, prefix, dir)
        self._ignore_cleanup_errors = ignore_cleanup_errors

    def cleanup(self):
        shutil.rmtree(self.name, ignore_errors=True)

    def __enter__(self):
        return self.name

    def __exit__(self, *exc):
        self.cleanup()
        return False


class NamedTemporaryFile:
    def __init__(self, mode="w+b", buffering=-1, encoding=None, newline=None, suffix=None, prefix=None, dir=None, delete=True):
        base = gettempdir() if dir is None else str(dir)
        os.makedirs(base, exist_ok=True)
        self.name = os.path.join(base, _next_name("tmp" if prefix is None else prefix, "" if suffix is None else suffix))
        self._delete = delete
        if "b" in mode:
            self.file = open(self.name, mode)
        else:
            self.file = open(self.name, mode, encoding=encoding, newline=newline)

    def __getattr__(self, name):
        return getattr(self.file, name)

    def close(self):
        self.file.close()
        if self._delete and os.path.exists(self.name):
            os.remove(self.name)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
        return False


def TemporaryFile(mode="w+b", buffering=-1, encoding=None, newline=None, suffix=None, prefix=None, dir=None):
    return io.BytesIO() if "b" in mode else io.StringIO()


def SpooledTemporaryFile(max_size=0, mode="w+b", buffering=-1, encoding=None, newline=None, suffix=None, prefix=None, dir=None):
    return TemporaryFile(mode=mode, buffering=buffering, encoding=encoding, newline=newline)
