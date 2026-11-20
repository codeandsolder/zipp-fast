"""PEP 654 except*, ExceptionGroup/BaseExceptionGroup, and PEP 678 add_note."""

# ---- PEP 678 add_note & __notes__ --------------------------------------------
e = ValueError("bad")
e.add_note("first note")
e.add_note("second note")
print("notes", e.__notes__)
try:
    e.add_note(123)
except TypeError as err:
    print("bad_note_type", type(err).__name__)

# ---- BaseExceptionGroup narrowing & validation -------------------------------
eg1 = BaseExceptionGroup("all exc", [ValueError("a"), TypeError("b")])
print("narrow", type(eg1).__name__, eg1.message, len(eg1.exceptions), repr(eg1))
eg2 = BaseExceptionGroup("mixed", [ValueError("a"), KeyboardInterrupt("k")])
print("base", type(eg2).__name__, eg2.message, len(eg2.exceptions), repr(eg2))
try:
    ExceptionGroup("bad", [KeyboardInterrupt("k")])
except TypeError as err:
    print("nest_base_in_eg", type(err).__name__)
try:
    ExceptionGroup("empty", [])
except ValueError as err:
    print("empty_eg", type(err).__name__)
try:
    ExceptionGroup(123, [ValueError("x")])
except TypeError as err:
    print("non_str_msg", type(err).__name__)

# ---- Nested split / subgroup / predicate / derive / notes preservation -------
inner = ExceptionGroup("inner", [ValueError("v2"), KeyError("k1")])
outer = ExceptionGroup("outer", [ValueError("v1"), inner, TypeError("t1")])
outer.add_note("outer-note")
m, u = outer.split(ValueError)
print("split_m", repr(m), getattr(m, "__notes__", None))
print("split_u", repr(u), getattr(u, "__notes__", None))
print("subgroup_key", repr(outer.subgroup(KeyError)))
print("subgroup_none", outer.subgroup(ZeroDivisionError))
print("subgroup_pred", repr(outer.subgroup(lambda x: isinstance(x, ValueError) and str(x) == "v2")))
print("subgroup_tuple", repr(outer.subgroup((KeyError, TypeError))))


class MyEG(ExceptionGroup):
    def __new__(cls, msg, excs, tag):
        obj = super().__new__(cls, msg, excs)
        obj.tag = tag
        return obj

    def derive(self, excs):
        return MyEG(self.message, excs, self.tag)


meg = MyEG("tagged", [ValueError("a"), TypeError("b")], 42)
sub = meg.subgroup(ValueError)
print("custom_derive", type(sub).__name__, sub.tag, repr(sub))

# ---- except* basic & multiple matching clauses -------------------------------
log = []
try:
    raise ExceptionGroup("g", [ValueError("v"), TypeError("t"), KeyError("k")])
except* ValueError as eg:
    log.append(("val", [str(x) for x in eg.exceptions]))
except* (TypeError, KeyError) as eg:
    log.append(("tk", [str(x) for x in eg.exceptions]))
else:
    log.append("else")
finally:
    log.append("finally")
print("multi_match", log)

# ---- except* else runs only when try completes normally ----------------------
log2 = []
try:
    log2.append("try")
except* ValueError:
    log2.append("caught")
else:
    log2.append("else")
finally:
    log2.append("finally")
print("normal_else", log2)

# ---- except* partial handling re-raises unhandled remainder ------------------
try:
    try:
        raise ExceptionGroup("outer", [ValueError(1), ExceptionGroup("inner", [TypeError(2), KeyError(3)])])
    except* TypeError as eg:
        print("partial_caught", repr(eg))
except ExceptionGroup as rem:
    print("partial_rem", repr(rem))

# ---- except* on plain (non-group) exception wraps it -------------------------
try:
    raise ValueError("solo")
except* ValueError as eg:
    print("solo_caught", type(eg).__name__, repr(eg))

# ---- except* bare reraise preserves original group structure -----------------
try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        raise
except ExceptionGroup as eg:
    print("reraise_all", repr(eg))

try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        raise
    except* TypeError:
        pass
except ExceptionGroup as eg:
    print("reraise_partial", repr(eg))

# ---- except* raising new exceptions inside handlers --------------------------
try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        raise RuntimeError("h1")
    except* TypeError:
        pass
except BaseException as err:
    print("handler_one_new", type(err).__name__, repr(err), type(err.__context__).__name__)

try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        raise RuntimeError("h1")
except BaseException as err:
    print("handler_new_plus_rem", type(err).__name__, [repr(x) for x in err.exceptions])

try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        raise RuntimeError("h1")
    except* TypeError:
        raise RuntimeError("h2")
except BaseException as err:
    print("handler_two_new", type(err).__name__, [repr(x) for x in err.exceptions])

# ---- except* forbids catching ExceptionGroup ---------------------------------
try:
    try:
        raise ValueError("x")
    except* ExceptionGroup:
        pass
except TypeError as err:
    print("forbid_eg", type(err).__name__)

# ---- except* unbinds `as` variable after handler -----------------------------
try:
    raise ValueError("u")
except* ValueError as bound_eg:
    print("bound_inside", len(bound_eg.exceptions))
try:
    _ = bound_eg
except NameError:
    print("unbound_after", True)

# ---- break/continue inside a loop NESTED within except* is allowed -----------
res = []
try:
    raise ExceptionGroup("loop", [ValueError(1)])
except* ValueError:
    for i in range(5):
        if i == 1:
            continue
        if i == 4:
            break
        res.append(i)
print("nested_loop_ok", res)
