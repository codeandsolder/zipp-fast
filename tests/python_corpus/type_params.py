"""PEP 695 type parameters / type aliases and PEP 696 type parameter defaults."""
import typing

# ---- Simple & generic type aliases (lazy __value__) --------------------------
type Simple = int | str
print("simple", type(Simple).__name__, Simple.__name__, Simple.__type_params__, Simple.__value__)

# Forward / recursive alias works because __value__ is evaluated lazily.
type Tree[T] = T | list[Tree[T]]
print("recursive", Tree.__name__, Tree.__type_params__, Tree.__value__, Tree[int])

# ---- Bounds, constraints, PEP 696 defaults -----------------------------------
type Bounded[T: int] = list[T]
Tb = Bounded.__type_params__[0]
print("bounded", Tb.__name__, Tb.__bound__, Tb.__constraints__, Tb.has_default(), Tb.__default__)

type Constrained[T: (int, str)] = list[T]
Tc = Constrained.__type_params__[0]
print("constrained", Tc.__name__, Tc.__bound__, Tc.__constraints__, Tc.has_default())

type WithDefault[T = int, *Ts = tuple[str, ...], **P = [int, str]] = tuple[T, *Ts]
t0, t1, t2 = WithDefault.__type_params__
print(
    "defaults",
    (type(t0).__name__, t0.__name__, t0.has_default(), t0.__default__),
    (type(t1).__name__, t1.__name__, t1.has_default(), t1.__default__),
    (type(t2).__name__, t2.__name__, t2.has_default(), t2.__default__),
)

# ---- Generic functions -------------------------------------------------------
default_val = 99


def first[T](items: list[T], fallback: T = default_val) -> T:
    return items[0] if items else fallback


print("fn_call", first([10, 20]), first([]))
print("fn_tp", [p.__name__ for p in first.__type_params__], first.__annotations__["return"] is first.__type_params__[0])


def variadic[T, *Ts, **P](x: T, *args: *Ts) -> tuple[T, *Ts]:
    return (x, *args)


print("variadic_call", variadic(1, "a", True))
print("variadic_tp", [type(p).__name__ for p in variadic.__type_params__])
print("variadic_ann", variadic.__annotations__)

# ---- Generic classes & subclasses --------------------------------------------
class Box[T = str]:
    def __init__(self, value: T):
        self.value = value

    def get(self) -> T:
        return self.value


class LabeledBox[U](Box[U]):
    def __init__(self, value: U, label: str):
        super().__init__(value)
        self.label = label

    def describe(self) -> tuple[str, U]:
        return (self.label, super().get())


print("box_meta", Box.__type_params__, Box.__orig_bases__, [c.__name__ for c in Box.__mro__])
print("box_alias", Box[int], Box[int].__origin__ is Box, Box[int].__args__, Box[int](7).get())
print("child_meta", LabeledBox.__type_params__, LabeledBox.__orig_bases__, [c.__name__ for c in LabeledBox.__mro__])
lb = LabeledBox[int](42, "ans")
print("child_inst", lb.describe(), isinstance(lb, Box), isinstance(lb, typing.Generic))

# ---- Generic method inside a class using super() -----------------------------
class Base:
    def tag(self, x):
        return f"base:{x}"


class Derived(Base):
    def tag[T](self, x: T) -> str:
        return "derived:" + super().tag(x)


print("generic_super", Derived().tag(5), Derived.tag.__type_params__[0].__name__)

# ---- Scoping: type parameter does not leak to outer scope --------------------
T = "outer"


def scoped[T](x: T) -> T:
    return (x, T.__name__)


print("scoping", T, scoped(3), T)

# ---- Builtin GenericAlias & union --------------------------------------------
ga = dict[str, list[int]]
print("builtin_ga", ga, ga.__origin__ is dict, ga.__args__, dict[str, int](a=1))
u = list[int] | None
print("ga_union", u, type(u).__name__)

# ---- Plain functions & classes have empty __type_params__ --------------------
def plain():
    pass


class Plain:
    pass


print("plain_tp", plain.__type_params__, Plain.__type_params__)
