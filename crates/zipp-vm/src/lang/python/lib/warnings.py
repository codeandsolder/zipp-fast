"""warnings module for Zipp Python."""
import re
import sys

filters = []
defaultaction = "default"
onceregistry = {}
_filters_mutated_count = 0


def _filters_mutated():
    global _filters_mutated_count
    _filters_mutated_count += 1


class WarningMessage:
    _WARNING_DETAILS = (
        "message",
        "category",
        "filename",
        "lineno",
        "file",
        "line",
        "source",
    )

    def __init__(self, message, category, filename, lineno, file=None, line=None, source=None):
        self.message = message
        self.category = category
        self.filename = filename
        self.lineno = lineno
        self.file = file
        self.line = line
        self.source = source
        self._category_name = category.__name__ if category is not None else None

    def __str__(self):
        return (
            "{message : %r, category : %r, filename : %r, lineno : %s, line : %r}"
            % (self.message, self._category_name, self.filename, self.lineno, self.line)
        )


def formatwarning(message, category, filename, lineno, line=None):
    msg = "%s:%s: %s: %s\n" % (filename, lineno, category.__name__, message)
    if line is not None:
        stripped = line.strip()
        if stripped:
            msg += "  %s\n" % stripped
    return msg


def showwarning(message, category, filename, lineno, file=None, line=None):
    target = sys.stderr if file is None else file
    if target is None:
        return
    try:
        target.write(formatwarning(message, category, filename, lineno, line))
    except Exception:
        pass


def filterwarnings(action, message="", category=Warning, module="", lineno=0, append=False):
    if action not in ("error", "ignore", "always", "default", "module", "once"):
        raise ValueError("invalid action: %r" % (action,))
    if not isinstance(message, str):
        raise TypeError("message must be a string")
    if not isinstance(category, type) or not issubclass(category, Warning):
        raise TypeError("category must be a Warning subclass")
    if not isinstance(module, str):
        raise TypeError("module must be a string")
    if not isinstance(lineno, int) or lineno < 0:
        raise ValueError("lineno must be an int >= 0")
    msg_re = re.compile(message, re.I) if message else None
    mod_re = re.compile(module) if module else None
    item = (action, msg_re, category, mod_re, int(lineno))
    if append:
        if item not in filters:
            filters.append(item)
    else:
        if item in filters:
            filters.remove(item)
        filters.insert(0, item)
    _filters_mutated()


def simplefilter(action, category=Warning, lineno=0, append=False):
    filterwarnings(action, "", category, "", lineno, append)


def resetwarnings():
    filters[:] = []
    _filters_mutated()


def _processoptions(args):
    for arg in args:
        parts = arg.split(":")
        if len(parts) > 5:
            continue
        while len(parts) < 5:
            parts.append("")
        action, message, cat_name, module, lineno_s = [p.strip() for p in parts]
        if not action:
            continue
        action_map = {
            "i": "ignore",
            "ignore": "ignore",
            "d": "default",
            "default": "default",
            "a": "always",
            "always": "always",
            "m": "module",
            "module": "module",
            "o": "once",
            "once": "once",
            "e": "error",
            "error": "error",
        }
        act = action_map.get(action.lower())
        if act is None:
            continue
        cat = Warning
        if cat_name:
            import builtins
            cand = getattr(builtins, cat_name, None)
            if isinstance(cand, type) and issubclass(cand, Warning):
                cat = cand
        lineno = int(lineno_s) if lineno_s.isdigit() else 0
        filterwarnings(act, re.escape(message) if message else "", cat, re.escape(module) if module else "", lineno)


def warn(message, category=None, stacklevel=1, source=None, *, skip_file_prefixes=()):
    if isinstance(message, Warning):
        category = message.__class__
    if category is None:
        category = UserWarning
    if not (isinstance(category, type) and issubclass(category, Warning)):
        raise TypeError("category must be a Warning sub-class, not '%s'" % type(category).__name__)
    filename = "<unknown>"
    lineno = 1
    module = "__main__"
    warn_explicit(message, category, filename, lineno, module, None, None, source)


def warn_explicit(message, category, filename, lineno, module=None, registry=None, module_globals=None, source=None):
    lineno = int(lineno)
    if module is None:
        module = filename or "<unknown>"
        if module.endswith(".py"):
            module = module[:-3]
    if isinstance(message, Warning):
        text = str(message)
        category = message.__class__
    else:
        text = str(message)
        message = category(message)
    action = defaultaction
    for item in filters:
        act, msg_re, cat, mod_re, ln = item
        if (
            (msg_re is None or msg_re.match(text))
            and issubclass(category, cat)
            and (mod_re is None or mod_re.match(module))
            and (ln == 0 or lineno == ln)
        ):
            action = act
            break
    if action == "ignore":
        return
    if action == "error":
        raise message
    if action == "once":
        key = (text, category)
        if onceregistry.get(key):
            return
        onceregistry[key] = 1
    elif action == "always":
        pass
    elif action == "module":
        key = (text, category, module)
        if onceregistry.get(key):
            return
        onceregistry[key] = 1
    elif action == "default":
        key = (text, category, filename, lineno)
        if registry is not None:
            if registry.get(key):
                return
            registry[key] = 1
        else:
            if onceregistry.get(key):
                return
            onceregistry[key] = 1
    showwarning(message, category, filename, lineno)


class catch_warnings:
    def __init__(self, *, record=False, module=None, action=None, category=Warning, lineno=0, append=False):
        self._record = record
        self._module = sys.modules["warnings"] if module is None else module
        self._entered = False
        self._action = action
        self._category = category
        self._lineno = lineno
        self._append = append

    def __repr__(self):
        args = []
        if self._record:
            args.append("record=True")
        return "%s(%s)" % (type(self).__name__, ", ".join(args))

    def __enter__(self):
        if self._entered:
            raise RuntimeError("Cannot enter %r twice" % self)
        self._entered = True
        self._filters = self._module.filters
        self._module.filters = self._filters[:]
        self._module._filters_mutated()
        self._showwarning = self._module.showwarning
        if self._action is not None:
            self._module.simplefilter(self._action, self._category, self._lineno, self._append)
        if self._record:
            log = []
            def _record_warning(message, category, filename, lineno, file=None, line=None):
                log.append(WarningMessage(message, category, filename, lineno, file, line))
            self._module.showwarning = _record_warning
            return log
        return None

    def __exit__(self, *exc_info):
        if not self._entered:
            raise RuntimeError("Cannot exit %r without entering first" % self)
        self._module.filters = self._filters
        self._module._filters_mutated()
        self._module.showwarning = self._showwarning
        return False


class deprecated:
    def __init__(self, message, /, *, category=DeprecationWarning, stacklevel=1):
        self.message = message
        self.category = category
        self.stacklevel = stacklevel

    def __call__(self, arg):
        msg = self.message
        cat = self.category
        sl = self.stacklevel
        if cat is None:
            return arg
        if isinstance(arg, type):
            orig_new = arg.__new__
            def _new(cls, *args, **kwargs):
                warn(msg, category=cat, stacklevel=sl + 1)
                if orig_new is object.__new__:
                    return orig_new(cls)
                return orig_new(cls, *args, **kwargs)
            arg.__new__ = _new
            return arg
        def _wrapped(*args, **kwargs):
            warn(msg, category=cat, stacklevel=sl + 1)
            return arg(*args, **kwargs)
        _wrapped.__name__ = getattr(arg, "__name__", "_wrapped")
        _wrapped.__doc__ = getattr(arg, "__doc__", None)
        return _wrapped


if getattr(sys, "warnoptions", None):
    _processoptions(sys.warnoptions)
