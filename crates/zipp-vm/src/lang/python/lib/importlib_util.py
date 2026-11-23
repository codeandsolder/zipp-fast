"""Utilities for constructing module specs and loading Python source files."""

import importlib
import os
import sys
import types

MAGIC_NUMBER = b"\xcb\x0d\x0d\x0a"

_UNSET = object()


def resolve_name(name, package):
    """Resolve a relative module name to an absolute dotted name."""
    if not name.startswith("."):
        return name
    if not package:
        raise ImportError(
            "no package specified for {!r} (required for relative module names)".format(name)
        )
    level = 0
    for ch in name:
        if ch != ".":
            break
        level += 1
    bits = package.rsplit(".", level - 1)
    if len(bits) < level:
        raise ImportError("attempted relative import beyond top-level package")
    base = bits[0]
    tail = name[level:]
    if not tail:
        return base
    return "{}.{}".format(base, tail)


class ModuleSpec:
    """Description of how to load a module."""

    def __init__(self, name, loader, *, origin=None, loader_state=None, is_package=None):
        self.name = name
        self.loader = loader
        self.origin = origin
        self.loader_state = loader_state
        self.submodule_search_locations = [] if is_package else None
        self._set_fileattr = False
        self._cached = None

    @property
    def cached(self):
        return self._cached

    @cached.setter
    def cached(self, value):
        self._cached = value

    @property
    def parent(self):
        if self.submodule_search_locations is None:
            return self.name.rpartition(".")[0]
        return self.name

    @property
    def has_location(self):
        return self._set_fileattr

    @has_location.setter
    def has_location(self, value):
        self._set_fileattr = bool(value)

    def __repr__(self):
        parts = ["name={!r}".format(self.name), "loader={!r}".format(self.loader)]
        if self.origin is not None:
            parts.append("origin={!r}".format(self.origin))
        if self.submodule_search_locations is not None:
            parts.append(
                "submodule_search_locations={!r}".format(self.submodule_search_locations)
            )
        return "{}({})".format(type(self).__name__, ", ".join(parts))

    def __eq__(self, other):
        if not isinstance(other, ModuleSpec):
            return NotImplemented
        return (
            self.name == other.name
            and self.loader == other.loader
            and self.origin == other.origin
            and self.submodule_search_locations == other.submodule_search_locations
            and self.has_location == other.has_location
        )


class SourceFileLoader:
    """Loader that compiles and executes a Python source file."""

    def __init__(self, fullname, path):
        self.name = fullname
        self.path = os.fspath(path) if hasattr(os, "fspath") else str(path)

    def __repr__(self):
        return "{}({!r}, {!r})".format(type(self).__name__, self.name, self.path)

    def __eq__(self, other):
        return (
            isinstance(other, SourceFileLoader)
            and self.name == other.name
            and self.path == other.path
        )

    def get_filename(self, fullname=None):
        if fullname is not None and fullname != self.name:
            raise ImportError(
                "loader for {} cannot handle {}".format(self.name, fullname),
                name=fullname,
            )
        return self.path

    def get_data(self, path):
        with open(path, "rb") as fh:
            return fh.read()

    def get_source(self, fullname=None):
        path = self.get_filename(fullname)
        with open(path, "r", encoding="utf-8") as fh:
            return fh.read()

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        source = self.get_source(self.name)
        mod_name = getattr(module, "__name__", self.name)
        if not getattr(module, "__file__", None):
            module.__file__ = self.path
        if getattr(module, "__loader__", None) is None:
            module.__loader__ = self
        importlib._exec_source(module, mod_name, source, self.path)


class _ImportLoader:
    """Loader delegating to importlib.import_module for built-in/bundled modules."""

    def __init__(self, fullname):
        self.name = fullname

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        loaded = importlib.import_module(self.name)
        if loaded is not module:
            for key in dir(loaded):
                if key not in ("__spec__", "__loader__"):
                    try:
                        setattr(module, key, getattr(loaded, key))
                    except Exception:
                        pass


def spec_from_loader(name, loader, *, origin=None, is_package=None):
    if is_package is None:
        if hasattr(loader, "is_package"):
            try:
                is_package = loader.is_package(name)
            except ImportError:
                is_package = False
        else:
            is_package = False
    spec = ModuleSpec(name, loader, origin=origin, is_package=is_package)
    if origin is not None:
        spec.has_location = True
    return spec


def spec_from_file_location(
    name, location=None, *, loader=None, submodule_search_locations=_UNSET
):
    if location is None:
        location = "<unknown>"
        if hasattr(loader, "get_filename"):
            try:
                location = loader.get_filename(name)
            except ImportError:
                pass
    else:
        location = os.fspath(location) if hasattr(os, "fspath") else str(location)
        if loader is None:
            loader = SourceFileLoader(name, location)

    spec = ModuleSpec(name, loader, origin=location)
    spec.has_location = True

    if submodule_search_locations is _UNSET:
        base = os.path.basename(location)
        stem = base.rsplit(".", 1)[0] if "." in base else base
        if stem == "__init__":
            spec.submodule_search_locations = [os.path.dirname(location)]
    elif submodule_search_locations is not None:
        spec.submodule_search_locations = list(submodule_search_locations)

    return spec


def module_from_spec(spec):
    """Create a module object populated from a ModuleSpec."""
    module = None
    if hasattr(spec.loader, "create_module"):
        module = spec.loader.create_module(spec)
    if module is None:
        module = types.ModuleType(spec.name)
    module.__name__ = spec.name
    module.__loader__ = spec.loader
    module.__package__ = spec.parent
    module.__spec__ = spec
    if spec.submodule_search_locations is not None:
        module.__path__ = list(spec.submodule_search_locations)
    if spec.has_location:
        module.__file__ = spec.origin
    return module


def find_spec(name, package=None):
    """Locate a ModuleSpec for *name* without executing the module if on disk."""
    fullname = resolve_name(name, package) if name.startswith(".") else name
    if fullname in sys.modules:
        mod = sys.modules[fullname]
        if mod is None:
            return None
        spec = getattr(mod, "__spec__", None)
        if spec is not None:
            return spec
        origin = getattr(mod, "__file__", None)
        is_pkg = hasattr(mod, "__path__")
        s = ModuleSpec(fullname, _ImportLoader(fullname), origin=origin, is_package=is_pkg)
        if origin is not None:
            s.has_location = True
        return s

    rel_parts = fullname.split(".")
    search_roots = list(sys.path)
    if "" not in search_roots and "." not in search_roots:
        search_roots.insert(0, "")

    for root in search_roots:
        prefix = os.path.join(root, *rel_parts) if root else os.path.join(*rel_parts)
        init_path = os.path.join(prefix, "__init__.py")
        if os.path.isfile(init_path):
            return spec_from_file_location(
                fullname, init_path, submodule_search_locations=[prefix]
            )
        file_path = prefix + ".py"
        if os.path.isfile(file_path):
            return spec_from_file_location(fullname, file_path)

    return None


class LazyLoader:
    """Wrapper loader that executes a module on first attribute access."""

    @classmethod
    def factory(cls, loader):
        return lambda *args, **kwargs: cls(loader(*args, **kwargs))

    def __init__(self, loader):
        self.loader = loader

    def create_module(self, spec):
        return self.loader.create_module(spec)

    def exec_module(self, module):
        self.loader.exec_module(module)
