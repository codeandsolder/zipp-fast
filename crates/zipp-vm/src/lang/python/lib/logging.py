"""Minimal standard-compatible logging module for Zipp Python."""
import sys

CRITICAL = 50
FATAL = CRITICAL
ERROR = 40
WARNING = 30
WARN = WARNING
INFO = 20
DEBUG = 10
NOTSET = 0

_levelToName = {
    CRITICAL: "CRITICAL",
    ERROR: "ERROR",
    WARNING: "WARNING",
    INFO: "INFO",
    DEBUG: "DEBUG",
    NOTSET: "NOTSET",
}
_nameToLevel = {v: k for k, v in _levelToName.items()}
_nameToLevel["WARN"] = WARNING
_nameToLevel["FATAL"] = CRITICAL


def getLevelName(level):
    if isinstance(level, int):
        return _levelToName.get(level, "Level %s" % level)
    return _nameToLevel.get(level, "Level %s" % level)


def addLevelName(level, levelName):
    _levelToName[level] = levelName
    _nameToLevel[levelName] = level


def _checkLevel(level):
    if isinstance(level, int):
        return level
    if isinstance(level, str) and level in _nameToLevel:
        return _nameToLevel[level]
    raise ValueError("Unknown level: %r" % (level,))


class LogRecord:
    def __init__(self, name, level, pathname, lineno, msg, args, exc_info, func=None, sinfo=None):
        self.name = name
        self.levelno = level
        self.levelname = getLevelName(level)
        self.pathname = pathname
        self.lineno = lineno
        self.msg = msg
        self.args = args
        self.exc_info = exc_info
        self.funcName = func
        self.stack_info = sinfo

    def getMessage(self):
        msg = str(self.msg)
        if self.args:
            msg = msg % self.args
        return msg


class Formatter:
    def __init__(self, fmt=None, datefmt=None, style="%"):
        self._fmt = fmt or "%(levelname)s:%(name)s:%(message)s"
        self.datefmt = datefmt

    def format(self, record):
        msg = record.getMessage()
        try:
            return self._fmt % {
                "levelname": record.levelname,
                "levelno": record.levelno,
                "name": record.name,
                "message": msg,
                "pathname": record.pathname,
                "lineno": record.lineno,
            }
        except Exception:
            return "%s:%s:%s" % (record.levelname, record.name, msg)


class Filter:
    def __init__(self, name=""):
        self.name = name

    def filter(self, record):
        if not self.name:
            return True
        return record.name == self.name or record.name.startswith(self.name + ".")


class Handler:
    def __init__(self, level=NOTSET):
        self.level = _checkLevel(level)
        self.formatter = None
        self.filters = []

    def setLevel(self, level):
        self.level = _checkLevel(level)

    def setFormatter(self, fmt):
        self.formatter = fmt

    def addFilter(self, flt):
        if flt not in self.filters:
            self.filters.append(flt)

    def removeFilter(self, flt):
        if flt in self.filters:
            self.filters.remove(flt)

    def filter(self, record):
        for f in self.filters:
            if callable(f):
                if not f(record):
                    return False
            elif not f.filter(record):
                return False
        return True

    def format(self, record):
        fmt = self.formatter if self.formatter is not None else Formatter()
        return fmt.format(record)

    def emit(self, record):
        raise NotImplementedError

    def handle(self, record):
        if self.filter(record):
            self.emit(record)
            return True
        return False

    def flush(self):
        pass

    def close(self):
        pass


class StreamHandler(Handler):
    terminator = "\n"

    def __init__(self, stream=None):
        super().__init__()
        self.stream = sys.stderr if stream is None else stream

    def emit(self, record):
        try:
            msg = self.format(record)
            if self.stream is not None:
                self.stream.write(msg + self.terminator)
        except Exception:
            pass


class NullHandler(Handler):
    def handle(self, record):
        pass

    def emit(self, record):
        pass


_disabled_level = NOTSET


class Logger:
    def __init__(self, name, level=NOTSET):
        self.name = name
        self.level = _checkLevel(level)
        self.parent = None
        self.propagate = True
        self.handlers = []
        self.filters = []
        self.disabled = False

    def setLevel(self, level):
        self.level = _checkLevel(level)

    def getEffectiveLevel(self):
        cur = self
        while cur is not None:
            if cur.level:
                return cur.level
            cur = cur.parent
        return NOTSET

    def isEnabledFor(self, level):
        if self.disabled or level <= _disabled_level:
            return False
        return level >= self.getEffectiveLevel()

    def addHandler(self, hdlr):
        if hdlr not in self.handlers:
            self.handlers.append(hdlr)

    def removeHandler(self, hdlr):
        if hdlr in self.handlers:
            self.handlers.remove(hdlr)

    def hasHandlers(self):
        cur = self
        while cur is not None:
            if cur.handlers:
                return True
            if not cur.propagate:
                break
            cur = cur.parent
        return False

    def addFilter(self, flt):
        if flt not in self.filters:
            self.filters.append(flt)

    def removeFilter(self, flt):
        if flt in self.filters:
            self.filters.remove(flt)

    def _log(self, level, msg, args, exc_info=None, extra=None, stack_info=False, stacklevel=1):
        if not self.isEnabledFor(level):
            return
        record = LogRecord(self.name, level, "<unknown>", 0, msg, args, exc_info)
        self.handle(record)

    def handle(self, record):
        if self.disabled:
            return
        for f in self.filters:
            if (f(record) if callable(f) else f.filter(record)) is False:
                return
        c = self
        found = 0
        while c is not None:
            for h in c.handlers:
                found += 1
                if record.levelno >= h.level:
                    h.handle(record)
            if not c.propagate:
                break
            c = c.parent
        if found == 0 and record.levelno >= WARNING:
            lastResort.handle(record)

    def debug(self, msg, *args, **kwargs):
        self._log(DEBUG, msg, args, **kwargs)

    def info(self, msg, *args, **kwargs):
        self._log(INFO, msg, args, **kwargs)

    def warning(self, msg, *args, **kwargs):
        self._log(WARNING, msg, args, **kwargs)

    warn = warning

    def error(self, msg, *args, **kwargs):
        self._log(ERROR, msg, args, **kwargs)

    def exception(self, msg, *args, exc_info=True, **kwargs):
        self.error(msg, *args, exc_info=exc_info, **kwargs)

    def critical(self, msg, *args, **kwargs):
        self._log(CRITICAL, msg, args, **kwargs)

    fatal = critical

    def log(self, level, msg, *args, **kwargs):
        self._log(_checkLevel(level), msg, args, **kwargs)


lastResort = StreamHandler()
lastResort.setLevel(WARNING)

root = Logger("root", WARNING)
_loggers = {}


def getLogger(name=None):
    if not name or name == "root":
        return root
    lg = _loggers.get(name)
    if lg is None:
        lg = Logger(name)
        dot = name.rfind(".")
        lg.parent = getLogger(name[:dot]) if dot > 0 else root
        _loggers[name] = lg
    return lg


def basicConfig(**kwargs):
    if kwargs.get("force"):
        root.handlers[:] = []
    if not root.handlers:
        handlers = kwargs.get("handlers")
        if handlers is None:
            h = StreamHandler(kwargs.get("stream"))
            if "format" in kwargs:
                h.setFormatter(Formatter(kwargs["format"], kwargs.get("datefmt")))
            root.addHandler(h)
        else:
            for h in handlers:
                root.addHandler(h)
    if "level" in kwargs and kwargs["level"] is not None:
        root.setLevel(kwargs["level"])


def disable(level=CRITICAL):
    global _disabled_level
    _disabled_level = _checkLevel(level)


def debug(msg, *args, **kwargs):
    root.debug(msg, *args, **kwargs)


def info(msg, *args, **kwargs):
    root.info(msg, *args, **kwargs)


def warning(msg, *args, **kwargs):
    root.warning(msg, *args, **kwargs)


warn = warning


def error(msg, *args, **kwargs):
    root.error(msg, *args, **kwargs)


def critical(msg, *args, **kwargs):
    root.critical(msg, *args, **kwargs)


def exception(msg, *args, exc_info=True, **kwargs):
    root.exception(msg, *args, exc_info=exc_info, **kwargs)


def log(level, msg, *args, **kwargs):
    root.log(level, msg, *args, **kwargs)
