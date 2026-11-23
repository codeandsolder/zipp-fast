"""Mock objects, call assertions, and attribute/dictionary patching for tests."""

import functools
import importlib
import inspect
import io


class _SentinelObject:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return "sentinel.{}".format(self.name)


class _Sentinel:
    def __init__(self):
        self._items = {}

    def __getattr__(self, name):
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        item = self._items.get(name)
        if item is None:
            item = _SentinelObject(name)
            self._items[name] = item
        return item


sentinel = _Sentinel()
DEFAULT = sentinel.DEFAULT


class _ANY:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return False

    def __repr__(self):
        return "<ANY>"


ANY = _ANY()


class _Call(tuple):
    def __new__(cls, value=(), name="", parent=None, two=False, from_kall=True):
        args = ()
        kwargs = {}
        if len(value) == 3:
            name, args, kwargs = value
        elif len(value) == 2:
            first, second = value
            if isinstance(first, str):
                name = first
                if isinstance(second, tuple):
                    args = second
                else:
                    kwargs = second
            else:
                args, kwargs = first, second
        elif len(value) == 1:
            (first,) = value
            if isinstance(first, str):
                name = first
            elif isinstance(first, tuple):
                args = first
            else:
                kwargs = first
        if two:
            obj = tuple.__new__(cls, (args, kwargs))
        else:
            obj = tuple.__new__(cls, (name, args, kwargs))
        obj._name = name
        obj._parent = parent
        obj._from_kall = from_kall
        return obj

    @property
    def args(self):
        return self[0] if len(self) == 2 else self[1]

    @property
    def kwargs(self):
        return self[1] if len(self) == 2 else self[2]

    def __eq__(self, other):
        if other is ANY:
            return True
        if not isinstance(other, tuple):
            return False
        if len(self) == 2:
            self_name, self_args, self_kwargs = "", self[0], self[1]
        else:
            self_name, self_args, self_kwargs = self[0], self[1], self[2]
        if len(other) == 2:
            o_first, o_second = other
            if isinstance(o_first, str):
                if isinstance(o_second, dict):
                    other_name, other_args, other_kwargs = o_first, (), o_second
                else:
                    other_name, other_args, other_kwargs = o_first, o_second, {}
            else:
                other_name, other_args, other_kwargs = "", o_first, o_second
        elif len(other) == 3:
            other_name, other_args, other_kwargs = other
        elif len(other) == 1:
            (o_first,) = other
            if isinstance(o_first, str):
                other_name, other_args, other_kwargs = o_first, (), {}
            elif isinstance(o_first, tuple):
                other_name, other_args, other_kwargs = "", o_first, {}
            elif isinstance(o_first, dict):
                other_name, other_args, other_kwargs = "", (), o_first
            else:
                return False
        else:
            return False
        return (
            self_name == other_name
            and self_args == other_args
            and self_kwargs == other_kwargs
        )

    def __ne__(self, other):
        return not self.__eq__(other)

    def __call__(self, *args, **kwargs):
        if self._name == "":
            return _Call(("", args, kwargs), name="()")
        name = self._name + "()"
        return _Call((self._name, args, kwargs), name=name, parent=self)

    def __getattr__(self, attr):
        if self._name is None:
            return _Call(name=attr, from_kall=False)
        name = "{}.{}".format(self._name, attr) if self._name else attr
        return _Call(name=name, parent=self, from_kall=False)

    def __repr__(self):
        if not self._from_kall:
            name = self._name or "call"
            if name.startswith("()"):
                name = "call" + name
            return name
        if len(self) == 2:
            name = ""
            args, kwargs = self
        else:
            name, args, kwargs = self
        if not name:
            prefix = "call"
        elif name.startswith("()"):
            prefix = "call" + name
        else:
            prefix = "call." + name
        parts = [repr(a) for a in args]
        parts.extend("{}={!r}".format(k, v) for k, v in kwargs.items())
        return "{}({})".format(prefix, ", ".join(parts))


call = _Call(from_kall=False)


class _CallList(list):
    def __contains__(self, value):
        if not isinstance(value, list):
            return list.__contains__(self, value)
        n = len(value)
        if n == 0:
            return True
        for idx in range(len(self) - n + 1):
            if self[idx : idx + n] == value:
                return True
        return False


def _format_call(name, args, kwargs):
    parts = [repr(a) for a in args]
    parts.extend("{}={!r}".format(k, v) for k, v in kwargs.items())
    return "{}({})".format(name or "mock", ", ".join(parts))


def _spec_attrs(spec):
    if spec is None:
        return None
    if isinstance(spec, (list, tuple, set, frozenset)):
        return set(spec)
    return set(dir(spec))


class NonCallableMock:
    """Base mock recording attribute access and supporting child mocks."""

    def __init__(
        self,
        spec=None,
        wraps=None,
        name=None,
        spec_set=None,
        parent=None,
        _spec_state=None,
        _new_name="",
        _new_parent=None,
        _spec_as_instance=False,
        _eat_self=None,
        unsafe=False,
        **kwargs,
    ):
        if spec_set is not None:
            spec = spec_set
            spec_set = True
        d = object.__getattribute__(self, "__dict__")
        d["_mock_return_value"] = DEFAULT
        d["_mock_side_effect"] = None
        d["_mock_wraps"] = wraps
        d["_mock_name"] = name
        d["_mock_new_name"] = _new_name
        d["_mock_parent"] = parent
        d["_mock_new_parent"] = _new_parent
        d["_mock_children"] = {}
        d["_mock_called"] = False
        d["_mock_call_count"] = 0
        d["_mock_call_args"] = None
        d["_mock_call_args_list"] = _CallList()
        d["_mock_method_calls"] = _CallList()
        d["_mock_mock_calls"] = _CallList()
        d["_mock_spec"] = spec
        d["_mock_spec_set"] = bool(spec_set)
        d["_mock_methods"] = _spec_attrs(spec)
        d["_mock_unsafe"] = unsafe
        d["_mock_sealed"] = False
        if kwargs:
            self.configure_mock(**kwargs)

    def _get_child_mock(self, **kw):
        cls = type(self)
        if cls is NonCallableMock:
            cls = Mock
        elif cls is NonCallableMagicMock:
            cls = MagicMock
        return cls(**kw)

    def _extract_mock_name(self):
        parts = []
        cur = self
        seen = set()
        while cur is not None and id(cur) not in seen:
            seen.add(id(cur))
            cur_d = object.__getattribute__(cur, "__dict__")
            nm = cur_d.get("_mock_name")
            if nm:
                parts.append(nm)
                break
            new_nm = cur_d.get("_mock_new_name")
            if new_nm:
                parts.append(new_nm)
            cur = cur_d.get("_mock_new_parent")
        if not parts:
            return "mock"
        parts.reverse()
        out = parts[0]
        for seg in parts[1:]:
            if seg == "()":
                out += "()"
            else:
                out += "." + seg
        return out

    def __repr__(self):
        name = self._extract_mock_name()
        return "<{} name={!r} id='{}'>".format(type(self).__name__, name, id(self))

    @property
    def return_value(self):
        d = object.__getattribute__(self, "__dict__")
        rv = d["_mock_return_value"]
        if rv is DEFAULT:
            rv = self._get_child_mock(_new_parent=self, _new_name="()")
            d["_mock_return_value"] = rv
        return rv

    @return_value.setter
    def return_value(self, value):
        object.__getattribute__(self, "__dict__")["_mock_return_value"] = value

    @property
    def side_effect(self):
        return object.__getattribute__(self, "__dict__")["_mock_side_effect"]

    @side_effect.setter
    def side_effect(self, value):
        d = object.__getattribute__(self, "__dict__")
        if (
            value is not None
            and not callable(value)
            and not isinstance(value, BaseException)
            and not (isinstance(value, type) and issubclass(value, BaseException))
        ):
            d["_mock_side_effect"] = iter(value)
        else:
            d["_mock_side_effect"] = value

    @property
    def called(self):
        return object.__getattribute__(self, "__dict__")["_mock_called"]

    @called.setter
    def called(self, value):
        object.__getattribute__(self, "__dict__")["_mock_called"] = bool(value)

    @property
    def call_count(self):
        return object.__getattribute__(self, "__dict__")["_mock_call_count"]

    @call_count.setter
    def call_count(self, value):
        object.__getattribute__(self, "__dict__")["_mock_call_count"] = value

    @property
    def call_args(self):
        return object.__getattribute__(self, "__dict__")["_mock_call_args"]

    @call_args.setter
    def call_args(self, value):
        object.__getattribute__(self, "__dict__")["_mock_call_args"] = value

    @property
    def call_args_list(self):
        return object.__getattribute__(self, "__dict__")["_mock_call_args_list"]

    @property
    def method_calls(self):
        return object.__getattribute__(self, "__dict__")["_mock_method_calls"]

    @property
    def mock_calls(self):
        return object.__getattribute__(self, "__dict__")["_mock_mock_calls"]

    def __getattr__(self, name):
        d = object.__getattribute__(self, "__dict__")
        if name.startswith("__") and name.endswith("__"):
            raise AttributeError(name)
        if not d.get("_mock_unsafe") and (
            name.startswith("assert_") or name.startswith("assret")
        ):
            raise AttributeError(
                "{!r} is not a valid assertion. Use a spec for the mock if {!r} is meant to be an attribute.".format(
                    name, name
                )
            )
        methods = d.get("_mock_methods")
        if methods is not None and name not in methods:
            raise AttributeError("Mock object has no attribute {!r}".format(name))
        if d.get("_mock_sealed"):
            raise AttributeError("Cannot access {!r} on sealed mock".format(name))
        children = d["_mock_children"]
        if name in children:
            return children[name]
        child = self._get_child_mock(parent=self, _new_parent=self, _new_name=name)
        children[name] = child
        d[name] = child
        return child

    def __setattr__(self, name, value):
        d = object.__getattribute__(self, "__dict__")
        if not name.startswith("_mock_") and name not in (
            "return_value",
            "side_effect",
            "called",
            "call_count",
            "call_args",
            "call_args_list",
            "method_calls",
            "mock_calls",
        ):
            if d.get("_mock_sealed"):
                raise AttributeError("Cannot set attribute {!r} on sealed mock".format(name))
            methods = d.get("_mock_methods")
            if d.get("_mock_spec_set") and methods is not None and name not in methods:
                raise AttributeError("Mock object has no attribute {!r}".format(name))
        object.__setattr__(self, name, value)

    def configure_mock(self, **kwargs):
        for attr, val in kwargs.items():
            parts = attr.split(".")
            final = parts.pop()
            target = self
            for part in parts:
                target = getattr(target, part)
            setattr(target, final, val)

    def attach_mock(self, mock, attribute):
        d = object.__getattribute__(mock, "__dict__")
        d["_mock_parent"] = self
        d["_mock_new_parent"] = self
        d["_mock_new_name"] = attribute
        object.__getattribute__(self, "__dict__")["_mock_children"][attribute] = mock
        setattr(self, attribute, mock)

    def reset_mock(self, visited=None, *, return_value=False, side_effect=False):
        if visited is None:
            visited = set()
        if id(self) in visited:
            return
        visited.add(id(self))
        d = object.__getattribute__(self, "__dict__")
        d["_mock_called"] = False
        d["_mock_call_count"] = 0
        d["_mock_call_args"] = None
        d["_mock_call_args_list"] = _CallList()
        d["_mock_method_calls"] = _CallList()
        d["_mock_mock_calls"] = _CallList()
        if return_value:
            d["_mock_return_value"] = DEFAULT
        if side_effect:
            d["_mock_side_effect"] = None
        for child in d["_mock_children"].values():
            if isinstance(child, NonCallableMock):
                child.reset_mock(visited, return_value=return_value, side_effect=side_effect)
        rv = d["_mock_return_value"]
        if isinstance(rv, NonCallableMock):
            rv.reset_mock(visited, return_value=return_value, side_effect=side_effect)

    def assert_not_called(self):
        if self.call_count != 0:
            raise AssertionError(
                "Expected '{}' to not have been called. Called {} times.".format(
                    self._extract_mock_name(), self.call_count
                )
            )

    def assert_called(self):
        if self.call_count == 0:
            raise AssertionError(
                "Expected '{}' to have been called.".format(self._extract_mock_name())
            )

    def assert_called_once(self):
        if self.call_count != 1:
            raise AssertionError(
                "Expected '{}' to be called once. Called {} times.".format(
                    self._extract_mock_name(), self.call_count
                )
            )

    def assert_called_with(self, *args, **kwargs):
        expected = _Call((args, kwargs), two=True)
        if self.call_args is None:
            raise AssertionError(
                "expected call not found.\nExpected: {}\nActual: not called.".format(
                    _format_call(self._extract_mock_name(), args, kwargs)
                )
            )
        if self.call_args != expected:
            actual_args, actual_kwargs = self.call_args
            raise AssertionError(
                "expected call not found.\nExpected: {}\nActual: {}".format(
                    _format_call(self._extract_mock_name(), args, kwargs),
                    _format_call(self._extract_mock_name(), actual_args, actual_kwargs),
                )
            )

    def assert_called_once_with(self, *args, **kwargs):
        if self.call_count != 1:
            raise AssertionError(
                "Expected '{}' to be called once. Called {} times.".format(
                    self._extract_mock_name(), self.call_count
                )
            )
        self.assert_called_with(*args, **kwargs)

    def assert_any_call(self, *args, **kwargs):
        expected = _Call((args, kwargs), two=True)
        if expected not in self.call_args_list:
            raise AssertionError(
                "{} call not found".format(
                    _format_call(self._extract_mock_name(), args, kwargs)
                )
            )

    def assert_has_calls(self, calls, any_order=False):
        expected = list(calls)
        actual = list(self.mock_calls)
        if not any_order:
            if expected not in self.mock_calls:
                raise AssertionError(
                    "Calls not found.\nExpected: {!r}\nActual: {!r}".format(
                        expected, actual
                    )
                )
            return
        remaining = list(actual)
        for item in expected:
            found = False
            for idx, act in enumerate(remaining):
                if act == item:
                    del remaining[idx]
                    found = True
                    break
            if not found:
                raise AssertionError(
                    "{!r} not all found in call list {!r}".format(expected, actual)
                )


class Mock(NonCallableMock):
    """Callable mock object tracking calls, return values, and side effects."""

    def __init__(
        self,
        spec=None,
        side_effect=None,
        return_value=DEFAULT,
        wraps=None,
        name=None,
        spec_set=None,
        parent=None,
        _spec_state=None,
        _new_name="",
        _new_parent=None,
        **kwargs,
    ):
        super().__init__(
            spec=spec,
            wraps=wraps,
            name=name,
            spec_set=spec_set,
            parent=parent,
            _spec_state=_spec_state,
            _new_name=_new_name,
            _new_parent=_new_parent,
            **kwargs,
        )
        if return_value is not DEFAULT:
            self.return_value = return_value
        if side_effect is not None:
            self.side_effect = side_effect

    def _record_call(self, args, kwargs):
        d = object.__getattribute__(self, "__dict__")
        d["_mock_called"] = True
        d["_mock_call_count"] += 1
        c = _Call((args, kwargs), two=True)
        d["_mock_call_args"] = c
        d["_mock_call_args_list"].append(c)
        d["_mock_mock_calls"].append(_Call(("", args, kwargs)))

        parent = d.get("_mock_parent")
        new_name = d.get("_mock_new_name") or ""
        path = new_name
        cur = parent
        seen = {id(self)}
        while cur is not None and id(cur) not in seen:
            seen.add(id(cur))
            cur_d = object.__getattribute__(cur, "__dict__")
            entry = _Call((path, args, kwargs))
            cur_d["_mock_mock_calls"].append(entry)
            if path and not path.startswith("()"):
                cur_d["_mock_method_calls"].append(entry)
            p_name = cur_d.get("_mock_new_name") or ""
            if p_name:
                path = "{}.{}".format(p_name, path) if path else p_name
            cur = cur_d.get("_mock_parent")

    def _execute_call(self, args, kwargs):
        d = object.__getattribute__(self, "__dict__")
        effect = d["_mock_side_effect"]
        if effect is not None:
            if isinstance(effect, BaseException):
                raise effect
            if isinstance(effect, type) and issubclass(effect, BaseException):
                raise effect()
            if callable(effect):
                result = effect(*args, **kwargs)
                if result is not DEFAULT:
                    return result
            else:
                result = next(effect)
                if isinstance(result, BaseException):
                    raise result
                if isinstance(result, type) and issubclass(result, BaseException):
                    raise result()
                if result is not DEFAULT:
                    return result
        wraps = d["_mock_wraps"]
        if wraps is not None and d["_mock_return_value"] is DEFAULT:
            return wraps(*args, **kwargs)
        return self.return_value

    def __call__(self, *args, **kwargs):
        self._record_call(args, kwargs)
        return self._execute_call(args, kwargs)


class NonCallableMagicMock(NonCallableMock):
    """Non-callable mock implementing default Python magic methods."""

    def __len__(self):
        d = object.__getattribute__(self, "__dict__")
        if "__len__" in d and callable(d["__len__"]):
            return d["__len__"]()
        return 0

    def __bool__(self):
        d = object.__getattribute__(self, "__dict__")
        if "__bool__" in d and callable(d["__bool__"]):
            return bool(d["__bool__"]())
        return True

    def __iter__(self):
        d = object.__getattribute__(self, "__dict__")
        if "__iter__" in d and callable(d["__iter__"]):
            return d["__iter__"]()
        return iter(())

    def __contains__(self, item):
        d = object.__getattribute__(self, "__dict__")
        if "__contains__" in d and callable(d["__contains__"]):
            return bool(d["__contains__"](item))
        return False

    def __int__(self):
        return 1

    def __float__(self):
        return 1.0

    def __enter__(self):
        d = object.__getattribute__(self, "__dict__")
        if "__enter__" in d and callable(d["__enter__"]):
            return d["__enter__"]()
        return self._get_child_mock(parent=self, _new_parent=self, _new_name="__enter__")()

    def __exit__(self, *args):
        d = object.__getattribute__(self, "__dict__")
        if "__exit__" in d and callable(d["__exit__"]):
            return d["__exit__"](*args)
        return False

    def __getitem__(self, key):
        d = object.__getattribute__(self, "__dict__")
        if "__getitem__" in d and callable(d["__getitem__"]):
            return d["__getitem__"](key)
        return self._get_child_mock(parent=self, _new_parent=self, _new_name="__getitem__")(key)

    def __setitem__(self, key, value):
        d = object.__getattribute__(self, "__dict__")
        if "__setitem__" in d and callable(d["__setitem__"]):
            return d["__setitem__"](key, value)
        return None


class MagicMock(Mock, NonCallableMagicMock):
    """Callable mock implementing common Python magic methods."""


class PropertyMock(Mock):
    """Mock acting as a data descriptor for patching properties."""

    def __get__(self, obj, obj_type=None):
        return self()

    def __set__(self, obj, val):
        self(val)


class AsyncMock(MagicMock):
    """Mock whose calls return coroutines and track await assertions."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        d = object.__getattribute__(self, "__dict__")
        d["_mock_await_count"] = 0
        d["_mock_await_args"] = None
        d["_mock_await_args_list"] = _CallList()

    @property
    def await_count(self):
        return object.__getattribute__(self, "__dict__")["_mock_await_count"]

    @property
    def await_args(self):
        return object.__getattribute__(self, "__dict__")["_mock_await_args"]

    @property
    def await_args_list(self):
        return object.__getattribute__(self, "__dict__")["_mock_await_args_list"]

    async def __call__(self, *args, **kwargs):
        self._record_call(args, kwargs)
        d = object.__getattribute__(self, "__dict__")
        d["_mock_await_count"] += 1
        c = _Call((args, kwargs), two=True)
        d["_mock_await_args"] = c
        d["_mock_await_args_list"].append(c)
        res = self._execute_call(args, kwargs)
        if inspect.isawaitable(res):
            return await res
        return res

    def assert_awaited(self):
        if self.await_count == 0:
            raise AssertionError(
                "Expected '{}' to have been awaited.".format(self._extract_mock_name())
            )

    def assert_awaited_once(self):
        if self.await_count != 1:
            raise AssertionError(
                "Expected '{}' to be awaited once. Awaited {} times.".format(
                    self._extract_mock_name(), self.await_count
                )
            )

    def assert_awaited_with(self, *args, **kwargs):
        expected = _Call((args, kwargs), two=True)
        if self.await_args != expected:
            raise AssertionError(
                "expected await not found.\nExpected: {}\nActual: {!r}".format(
                    _format_call(self._extract_mock_name(), args, kwargs), self.await_args
                )
            )

    def assert_awaited_once_with(self, *args, **kwargs):
        self.assert_awaited_once()
        self.assert_awaited_with(*args, **kwargs)

    def assert_any_await(self, *args, **kwargs):
        expected = _Call((args, kwargs), two=True)
        if expected not in self.await_args_list:
            raise AssertionError("await call not found")

    def assert_not_awaited(self):
        if self.await_count != 0:
            raise AssertionError(
                "Expected '{}' to not have been awaited. Awaited {} times.".format(
                    self._extract_mock_name(), self.await_count
                )
            )


def _import_target(target):
    parts = target.split(".")
    if len(parts) < 2:
        raise TypeError("Need a valid target to patch. You supplied: {!r}".format(target))
    for split_at in range(len(parts) - 1, 0, -1):
        mod_name = ".".join(parts[:split_at])
        try:
            obj = importlib.import_module(mod_name)
        except ModuleNotFoundError:
            continue
        for attr in parts[split_at:-1]:
            obj = getattr(obj, attr)
        return obj, parts[-1]
    raise ModuleNotFoundError("No module named {!r}".format(parts[0]))


_ACTIVE_PATCHERS = []


class _Patcher:
    def __init__(
        self,
        getter,
        attribute,
        new=DEFAULT,
        spec=None,
        create=False,
        spec_set=None,
        autospec=None,
        new_callable=None,
        kwargs=None,
    ):
        self.getter = getter
        self.attribute = attribute
        self.new = new
        self.spec = spec
        self.create = create
        self.spec_set = spec_set
        self.autospec = autospec
        self.new_callable = new_callable
        self.kwargs = kwargs or {}
        self.attribute_name = attribute
        self._stack = []

    def _get_original(self, target):
        try:
            original = getattr(target, self.attribute)
            is_local = self.attribute in getattr(target, "__dict__", {})
            return original, is_local, True
        except AttributeError:
            if not self.create:
                raise AttributeError(
                    "{!r} does not have the attribute {!r}".format(target, self.attribute)
                )
            return DEFAULT, False, False

    def __enter__(self):
        target = self.getter()
        original, is_local, existed = self._get_original(target)
        if self.new is DEFAULT:
            if self.new_callable is not None:
                replacement = self.new_callable(**self.kwargs)
            elif self.autospec is not None:
                spec_src = original if self.autospec is True else self.autospec
                replacement = create_autospec(
                    spec_src, spec_set=bool(self.spec_set), **self.kwargs
                )
            else:
                mk_kw = dict(self.kwargs)
                if self.spec is True:
                    mk_kw["spec"] = original
                elif self.spec is not None:
                    mk_kw["spec"] = self.spec
                if self.spec_set is True:
                    mk_kw["spec_set"] = original
                elif self.spec_set is not None:
                    mk_kw["spec_set"] = self.spec_set
                mk_kw.setdefault("name", self.attribute)
                replacement = MagicMock(**mk_kw)
        else:
            replacement = self.new
        setattr(target, self.attribute, replacement)
        self._stack.append((target, original, is_local, existed))
        return replacement

    def __exit__(self, *exc_info):
        if not self._stack:
            return False
        target, original, is_local, existed = self._stack.pop()
        if existed and (is_local or not isinstance(target, type)):
            setattr(target, self.attribute, original)
        else:
            try:
                delattr(target, self.attribute)
            except AttributeError:
                if existed:
                    setattr(target, self.attribute, original)
        return False

    def start(self):
        res = self.__enter__()
        _ACTIVE_PATCHERS.append(self)
        return res

    def stop(self):
        if self in _ACTIVE_PATCHERS:
            _ACTIVE_PATCHERS.remove(self)
        return self.__exit__(None, None, None)

    def __call__(self, func):
        if isinstance(func, type):
            for attr in dir(func):
                if attr.startswith("test"):
                    val = getattr(func, attr)
                    if callable(val):
                        setattr(func, attr, self(val))
            return func

        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            with self as patched:
                if self.new is DEFAULT:
                    return func(*args, patched, **kwargs)
                return func(*args, **kwargs)

        return wrapper


class _PatchDict:
    def __init__(self, in_dict, values=(), clear=False, **kwargs):
        self.in_dict = in_dict
        self.values = dict(values)
        self.values.update(kwargs)
        self.clear = clear
        self._stack = []

    def _resolve(self):
        if isinstance(self.in_dict, str):
            parent, attr = _import_target(self.in_dict)
            return getattr(parent, attr)
        return self.in_dict

    def __enter__(self):
        target = self._resolve()
        saved = dict(target)
        self._stack.append((target, saved))
        if self.clear:
            target.clear()
        target.update(self.values)
        return target

    def __exit__(self, *exc_info):
        if self._stack:
            target, saved = self._stack.pop()
            target.clear()
            target.update(saved)
        return False

    def start(self):
        res = self.__enter__()
        _ACTIVE_PATCHERS.append(self)
        return res

    def stop(self):
        if self in _ACTIVE_PATCHERS:
            _ACTIVE_PATCHERS.remove(self)
        return self.__exit__(None, None, None)

    def __call__(self, func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            with self:
                return func(*args, **kwargs)

        return wrapper


class _PatchMultiple:
    def __init__(self, target, **kwargs):
        self.target = target
        self.kwargs = kwargs
        self._patchers = []

    def __enter__(self):
        obj = (
            importlib.import_module(self.target)
            if isinstance(self.target, str)
            else self.target
        )
        results = {}
        patchers = []
        try:
            for attr, val in self.kwargs.items():
                p = _Patcher(lambda o=obj: o, attr, new=val)
                results[attr] = p.__enter__()
                patchers.append(p)
        except Exception:
            for p in reversed(patchers):
                p.__exit__(None, None, None)
            raise
        self._patchers.append(patchers)
        return results

    def __exit__(self, *exc_info):
        if self._patchers:
            for p in reversed(self._patchers.pop()):
                p.__exit__(*exc_info)
        return False

    def start(self):
        res = self.__enter__()
        _ACTIVE_PATCHERS.append(self)
        return res

    def stop(self):
        if self in _ACTIVE_PATCHERS:
            _ACTIVE_PATCHERS.remove(self)
        return self.__exit__(None, None, None)

    def __call__(self, func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            with self as patched:
                kw = dict(kwargs)
                kw.update(patched)
                return func(*args, **kw)

        return wrapper


def patch(
    target,
    new=DEFAULT,
    spec=None,
    create=False,
    spec_set=None,
    autospec=None,
    new_callable=None,
    **kwargs,
):
    """Replace *target* (dotted 'module.attr' path) for the duration of a block or call."""
    if "." not in target:
        raise TypeError("Need a valid target to patch. You supplied: {!r}".format(target))
    attr = target.rsplit(".", 1)[1]
    return _Patcher(
        lambda: _import_target(target)[0],
        attr,
        new=new,
        spec=spec,
        create=create,
        spec_set=spec_set,
        autospec=autospec,
        new_callable=new_callable,
        kwargs=kwargs,
    )


def _patch_object(
    target,
    attribute,
    new=DEFAULT,
    spec=None,
    create=False,
    spec_set=None,
    autospec=None,
    new_callable=None,
    **kwargs,
):
    return _Patcher(
        lambda: target,
        attribute,
        new=new,
        spec=spec,
        create=create,
        spec_set=spec_set,
        autospec=autospec,
        new_callable=new_callable,
        kwargs=kwargs,
    )


def _patch_stopall():
    while _ACTIVE_PATCHERS:
        _ACTIVE_PATCHERS.pop().stop()


patch.object = _patch_object
patch.dict = _PatchDict
patch.multiple = _PatchMultiple
patch.stopall = _patch_stopall


def create_autospec(
    spec, spec_set=False, instance=False, _parent=None, _name=None, **kwargs
):
    is_callable = callable(spec)
    cls = MagicMock if is_callable else NonCallableMagicMock
    return cls(
        spec=spec if not spec_set else None,
        spec_set=spec if spec_set else None,
        parent=_parent,
        _new_parent=_parent,
        _new_name=_name or "",
        **kwargs,
    )


def seal(mock):
    d = object.__getattribute__(mock, "__dict__")
    d["_mock_sealed"] = True
    for child in d["_mock_children"].values():
        if isinstance(child, NonCallableMock):
            seal(child)


def mock_open(mock=None, read_data=""):
    """Build a mock simulating open() with in-memory *read_data*."""
    if mock is None:
        mock = MagicMock(name="open")

    is_bytes = isinstance(read_data, (bytes, bytearray))

    def _make_handle(*args, **kwargs):
        buf = io.BytesIO(read_data) if is_bytes else io.StringIO(read_data)
        handle = MagicMock(name="file")
        handle.read.side_effect = buf.read
        handle.readline.side_effect = buf.readline
        handle.readlines.side_effect = buf.readlines
        handle.write.return_value = None
        handle.__iter__ = MagicMock(side_effect=lambda: iter(buf))
        handle.__enter__ = MagicMock(return_value=handle)
        handle.__exit__ = MagicMock(return_value=False)
        return handle

    handle = _make_handle()
    mock.return_value = handle
    mock.side_effect = None
    return mock
