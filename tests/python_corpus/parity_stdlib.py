"""CPython 3.13 parity: __getattribute__, weakref, types, enum, abc, numbers, fractions, functools, itertools."""
import abc
import enum
import fractions
import functools
import itertools
import math
import numbers
import types
import weakref

# ---- __getattribute__ overrides ----------------------------------------------
log_ga = []


class Tracked:
    cls_val = 99

    def __init__(self, x):
        self.x = x

    def greet(self, who):
        return f"hi {who}:{self.x}"

    def __getattribute__(self, name):
        log_ga.append(name)
        if name == "secret":
            return 777
        return super().__getattribute__(name)

    def __getattr__(self, name):
        if name.startswith("dyn_"):
            return f"fallback:{name}"
        raise AttributeError(name)


tr = Tracked(10)
print("ga_attrs", tr.x, tr.cls_val, tr.secret, tr.dyn_ok, tr.greet("bob"))
print("ga_has", hasattr(tr, "secret"), hasattr(tr, "dyn_a"), hasattr(tr, "missing"), getattr(tr, "missing", "DEF"))
print("ga_log", log_ga[:6])


class MetaGA(type):
    def __getattribute__(cls, name):
        if name == "MAGIC":
            return "from_meta"
        return super().__getattribute__(name)


class WithMeta(metaclass=MetaGA):
    pass


print("meta_ga", WithMeta.MAGIC)

# ---- weakref -----------------------------------------------------------------
class Box:
    def __init__(self, v):
        self.v = v

    def __call__(self, x):
        return self.v + x


b1 = Box(10)
b2 = Box(20)
r1 = weakref.ref(b1)
r2 = weakref.ref(b1)
print("wr_ref", r1() is b1, r1 is r2, r1 == r2, hash(r1) == hash(b1), weakref.getweakrefcount(b1))
try:
    weakref.ref([1, 2])
except TypeError:
    print("wr_reject_list", True)

p1 = weakref.proxy(b1)
print("wr_proxy", p1.v, p1(5), isinstance(p1, weakref.CallableProxyType))
p1.v = 15
print("wr_proxy_set", b1.v)

wvd = weakref.WeakValueDictionary({"a": b1, "b": b2})
print("wvd", len(wvd), wvd["a"] is b1, "b" in wvd, sorted(wvd.keys()))
wkd = weakref.WeakKeyDictionary()
wkd[b1] = "first"
wkd[b2] = "second"
print("wkd", len(wkd), wkd[b1], b2 in wkd)
ws = weakref.WeakSet([b1, b2])
print("ws", len(ws), b1 in ws)

fin_log = []
fobj = weakref.finalize(b1, lambda tag: fin_log.append(tag), "cleaned")
print("fin_before", fobj.alive, fobj.peek()[0] is b1)
fobj()
print("fin_after", fobj.alive, fin_log, fobj.peek())

# ---- types -------------------------------------------------------------------
ns1 = types.SimpleNamespace(a=1, b="two")
ns2 = types.SimpleNamespace({"a": 1, "b": "two"})
ns3 = ns1.__replace__(b="THREE")
print("ns", ns1, ns1 == ns2, ns3, ns1 == ns3)
mp = types.MappingProxyType({"k": 42})
print("mp", mp["k"], "k" in mp, len(mp))
try:
    mp["k"] = 99
except TypeError:
    print("mp_readonly", True)


def _sample_fn():
    pass


def _sample_gen():
    yield 1


async def _sample_coro():
    return 1


async def _sample_agen():
    yield 1


c_obj = _sample_coro()
g_obj = _sample_gen()
ag_obj = _sample_agen()
print(
    "types_checks",
    isinstance(_sample_fn, types.FunctionType),
    isinstance(g_obj, types.GeneratorType),
    isinstance(c_obj, types.CoroutineType),
    isinstance(ag_obj, types.AsyncGeneratorType),
    isinstance(list[int], types.GenericAlias),
    isinstance(int | str, types.UnionType),
    isinstance(None, types.NoneType),
)
c_obj.close()
DynCls = types.new_class("DynCls", (), {}, lambda ns: ns.update({"x": 123}))
print("new_class", DynCls.__name__, DynCls.x)

# ---- enum parity (IntEnum, StrEnum, IntFlag, mixins) -------------------------
class Status(enum.IntEnum):
    OK = 200
    NOT_FOUND = 404


class HttpMethod(enum.StrEnum):
    GET = enum.auto()
    POST = enum.auto()


class Perm(enum.IntFlag):
    R = 4
    W = 2
    X = 1


class CustomStr(str, enum.Enum):
    ALPHA = "alpha"


print(
    "enum_int",
    isinstance(Status.OK, int),
    Status.OK + 1,
    Status.OK == 200,
    Status.OK < Status.NOT_FOUND,
    str(Status.OK),
    repr(Status.OK),
)
print(
    "enum_str",
    isinstance(HttpMethod.GET, str),
    HttpMethod.GET == "get",
    HttpMethod.POST.upper(),
    HttpMethod.GET + "/index",
    str(HttpMethod.GET),
    repr(HttpMethod.GET),
)
print(
    "enum_flag",
    isinstance(Perm.R, int),
    isinstance(Perm.R | Perm.W, int),
    (Perm.R | Perm.W) == 6,
    int(Perm.R | Perm.X),
)
print("enum_mixin", isinstance(CustomStr.ALPHA, str), CustomStr.ALPHA.upper(), CustomStr.ALPHA == "alpha")

# ---- abc.ABCMeta register & __subclasshook__ ---------------------------------
class MyABC(abc.ABC):
    pass


class VirtualImpl:
    pass


MyABC.register(VirtualImpl)
print("abc_reg", isinstance(VirtualImpl(), MyABC), issubclass(VirtualImpl, MyABC))

# ---- numbers & fractions -----------------------------------------------------
F = fractions.Fraction
print(
    "numbers_tower",
    isinstance(True, numbers.Integral),
    isinstance(5, numbers.Integral),
    isinstance(5, numbers.Real),
    isinstance(5, numbers.Number),
    isinstance(3.14, numbers.Real),
    isinstance(3.14, numbers.Rational),
    isinstance(1 + 2j, numbers.Complex),
    isinstance(F(3, 4), numbers.Rational),
    isinstance(F(3, 4), numbers.Real),
    issubclass(int, numbers.Complex),
)

f1 = F(6, 8)
f2 = F(" -1_000 / 4_000 ")
f3 = F("1.25")
f4 = F("2.5e-1")
f5 = F.from_float(0.5)
print("frac_init", repr(f1), str(f1), repr(f2), repr(f3), repr(f4), repr(f5))
print("frac_ops", f1 + f2, f1 - f2, f1 * f3, f1 / f5, F(7, 3) // 1, F(7, 3) % 1, divmod(F(7, 3), F(1, 2)), F(2, 3) ** -2)
print("frac_methods", F(6, 3).is_integer(), F(7, 3).is_integer(), F(3, 4).as_integer_ratio(), F(3.141592653589793).limit_denominator(100))
print("frac_round", round(F(5, 2)), round(F(7, 2)), round(F(123, 100), 1), math.floor(F(-7, 3)), math.ceil(F(-7, 3)), math.trunc(F(-7, 3)))
print("frac_cmp", F(3, 2) == 1.5, F(3, 2) == F(6, 4), F(4, 2) == 2, F(1, 3) < 0.5, F(1, 3) < float("inf"), F(1, 3) == float("nan"))
d_hash = {1.5: "hit_float", 5: "hit_int"}
print("frac_hash", hash(F(3, 2)) == hash(1.5), hash(F(5, 1)) == hash(5), d_hash[F(3, 2)], d_hash[F(5, 1)])

# ---- functools.singledispatch & cached_property ------------------------------
@functools.singledispatch
def fmt(x):
    return f"default:{x}"


@fmt.register(int)
def _(x):
    return f"int:{x * 2}"


@fmt.register
def _(x: str):
    return f"str:{x.upper()}"


@fmt.register(list | tuple)
def _(x):
    return f"seq:{len(x)}"


print("sd", fmt(3.5), fmt(21), fmt("hi"), fmt([1, 2, 3]), fmt((4, 5)), fmt.dispatch(bool)(True), int in fmt.registry)


class Computed:
    def __init__(self, n):
        self.n = n
        self.calls = 0

    @functools.cached_property
    def sq(self):
        self.calls += 1
        return self.n * self.n


comp = Computed(6)
v1 = comp.sq
v2 = comp.sq
comp.sq = 999
v3 = comp.sq
del comp.sq
v4 = comp.sq
print("cached_prop", v1, v2, v3, v4, comp.calls)

# ---- itertools.batched (Python 3.13 strict=True) -----------------------------
print("batched_ok", list(itertools.batched("ABCDE", 2)))
print("batched_strict_ok", list(itertools.batched("ABCD", 2, strict=True)))
try:
    list(itertools.batched("ABCDE", 2, strict=True))
except ValueError:
    print("batched_strict_err", True)
