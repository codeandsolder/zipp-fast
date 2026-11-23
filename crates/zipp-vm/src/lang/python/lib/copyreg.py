"""Helper registry for pickle/copy customization (`copyreg`)."""

__all__ = [
    "pickle", "constructor", "add_extension", "remove_extension",
    "clear_extension_cache", "dispatch_table",
]

dispatch_table = {}
_extension_registry = {}
_inverted_registry = {}
_extension_cache = {}


def pickle(ob_type, pickle_function, constructor_ob=None):
    if not callable(pickle_function):
        raise TypeError("reduction functions must be callable")
    dispatch_table[ob_type] = pickle_function
    if constructor_ob is not None:
        constructor(constructor_ob)


def constructor(object):
    if not callable(object):
        raise TypeError("constructors must be callable")


def _reconstructor(cls, base, state):
    if base is object:
        obj = object.__new__(cls)
    else:
        obj = base.__new__(cls, state)
        if base.__init__ != object.__init__:
            base.__init__(obj, state)
    return obj


def __newobj__(cls, *args):
    return cls.__new__(cls, *args)


def __newobj_ex__(cls, args, kwargs):
    return cls.__new__(cls, *args, **kwargs)


def _slotnames(cls):
    names = cls.__dict__.get("__slotnames__")
    if names is not None:
        return names
    names = []
    for c in cls.__mro__:
        slots = c.__dict__.get("__slots__")
        if not slots:
            continue
        if isinstance(slots, str):
            slots = (slots,)
        for name in slots:
            if name in ("__dict__", "__weakref__"):
                continue
            if name.startswith("__") and not name.endswith("__"):
                stripped = c.__name__.lstrip("_")
                if stripped:
                    name = "_%s%s" % (stripped, name)
            names.append(name)
    return names


def add_extension(module, name, code):
    code = int(code)
    if not 1 <= code <= 0x7FFFFFFF:
        raise ValueError("code out of range")
    key = (module, name)
    if _extension_registry.get(key) == code and _inverted_registry.get(code) == key:
        return
    if key in _extension_registry:
        raise ValueError("key %s is already registered with code %s" % (key, _extension_registry[key]))
    if code in _inverted_registry:
        raise ValueError("code %s is already in use for key %s" % (code, _inverted_registry[code]))
    _extension_registry[key] = code
    _inverted_registry[code] = key


def remove_extension(module, name, code):
    key = (module, name)
    if _extension_registry.get(key) != code or _inverted_registry.get(code) != key:
        raise ValueError("key %s is not registered with code %s" % (key, code))
    del _extension_registry[key]
    del _inverted_registry[code]
    _extension_cache.pop(code, None)


def clear_extension_cache():
    _extension_cache.clear()
