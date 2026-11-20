"""Exact rational arithmetic (`fractions.Fraction`)."""

import math
import numbers

__all__ = ["Fraction"]

_PyHASH_MODULUS = (1 << 61) - 1
_PyHASH_INF = 314159


def _float_ratio(f):
    if math.isnan(f):
        raise ValueError("cannot convert NaN to integer ratio")
    if math.isinf(f):
        raise OverflowError("cannot convert Infinity to integer ratio")
    if hasattr(f, "as_integer_ratio"):
        return f.as_integer_ratio()
    if f == 0.0:
        return (0, 1)
    neg = f < 0
    f = -f if neg else f
    m, e = math.frexp(f)
    for _ in range(300):
        if m == math.floor(m):
            break
        m *= 2.0
        e -= 1
    n = int(m)
    if neg:
        n = -n
    if e >= 0:
        return (n << e, 1)
    return (n, 1 << (-e))


def _parse_str(s):
    s_strip = s.strip()
    if not s_strip:
        raise ValueError(f"Invalid literal for Fraction: {s!r}")
    if "/" in s_strip:
        parts = s_strip.split("/")
        if len(parts) != 2:
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        p0 = parts[0].strip()
        p1 = parts[1].strip()
        if not p0 or not p1 or p1[0] in "+-":
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        try:
            num = int(p0)
            den = int(p1)
        except ValueError:
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        if den == 0:
            raise ZeroDivisionError(f"Fraction({num}, 0)")
        return num, den
    sign = 1
    body = s_strip
    if body[0] == "-":
        sign = -1
        body = body[1:]
    elif body[0] == "+":
        body = body[1:]
    if not body or body[0] in "+-":
        raise ValueError(f"Invalid literal for Fraction: {s!r}")
    exp = 0
    for sep in ("e", "E"):
        if sep in body:
            mant, exp_s = body.split(sep, 1)
            try:
                exp = int(exp_s)
            except ValueError:
                raise ValueError(f"Invalid literal for Fraction: {s!r}")
            body = mant
            break
    if "." in body:
        parts = body.split(".")
        if len(parts) != 2:
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        int_part, frac_part = parts
        if not int_part and not frac_part:
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        clean_frac = frac_part.replace("_", "")
        clean_int = int_part.replace("_", "") if int_part else "0"
        if not clean_int.isdigit() or (clean_frac and not clean_frac.isdigit()):
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        scale = len(clean_frac)
        num = int(clean_int) * (10 ** scale) + (int(clean_frac) if clean_frac else 0)
        den = 10 ** scale
    else:
        clean = body.replace("_", "")
        if not clean.isdigit():
            raise ValueError(f"Invalid literal for Fraction: {s!r}")
        num = int(clean)
        den = 1
    if exp > 0:
        num *= 10 ** exp
    elif exp < 0:
        den *= 10 ** (-exp)
    return sign * num, den


class Fraction(numbers.Rational):
    __slots__ = ("_numerator", "_denominator")

    def __new__(cls, numerator=0, denominator=None):
        self = object.__new__(cls)
        if denominator is None:
            if isinstance(numerator, int):
                self._numerator = int(numerator)
                self._denominator = 1
                return self
            if isinstance(numerator, Fraction):
                self._numerator = numerator._numerator
                self._denominator = numerator._denominator
                return self
            if isinstance(numerator, numbers.Rational):
                self._numerator = int(numerator.numerator)
                self._denominator = int(numerator.denominator)
                return self
            if isinstance(numerator, float):
                num, den = _float_ratio(numerator)
                self._numerator = num
                self._denominator = den
                return self
            if isinstance(numerator, str):
                num, den = _parse_str(numerator)
            else:
                raise TypeError("argument should be a string or a Rational instance")
        elif isinstance(numerator, int) and isinstance(denominator, int):
            num = int(numerator)
            den = int(denominator)
        elif isinstance(numerator, numbers.Rational) and isinstance(denominator, numbers.Rational):
            num = int(numerator.numerator) * int(denominator.denominator)
            den = int(numerator.denominator) * int(denominator.numerator)
        else:
            raise TypeError("both arguments should be Rational instances")

        if den == 0:
            raise ZeroDivisionError(f"Fraction({num}, 0)")
        if den < 0:
            num = -num
            den = -den
        g = math.gcd(abs(num), den)
        self._numerator = num // g
        self._denominator = den // g
        return self

    @classmethod
    def from_float(cls, f):
        if isinstance(f, int):
            return cls(f)
        if not isinstance(f, float):
            raise TypeError(f"{cls.__name__}.from_float() only takes floats, not {f!r} ({type(f).__name__})")
        return cls(*_float_ratio(f))

    @classmethod
    def from_decimal(cls, dec):
        if isinstance(dec, int):
            return cls(dec)
        if hasattr(dec, "as_integer_ratio"):
            return cls(*dec.as_integer_ratio())
        raise TypeError(f"{cls.__name__}.from_decimal() only takes Decimals, not {dec!r} ({type(dec).__name__})")

    def as_integer_ratio(self):
        return (self._numerator, self._denominator)

    def is_integer(self):
        return self._denominator == 1

    def limit_denominator(self, max_denominator=1000000):
        if max_denominator < 1:
            raise ValueError("max_denominator should be at least 1")
        if self._denominator <= max_denominator:
            return Fraction(self._numerator, self._denominator)
        p0, q0, p1, q1 = 0, 1, 1, 0
        n, d = self._numerator, self._denominator
        while True:
            a = n // d
            q2 = q0 + a * q1
            if q2 > max_denominator:
                break
            p0, q0, p1, q1 = p1, q1, p0 + a * p1, q2
            n, d = d, n - a * d
        k = (max_denominator - q0) // q1
        if 2 * d * (q0 + k * q1) <= self._denominator:
            return Fraction(p1, q1)
        return Fraction(p0 + k * p1, q0 + k * q1)

    @property
    def numerator(self):
        return self._numerator

    @property
    def denominator(self):
        return self._denominator

    def __repr__(self):
        return f"{type(self).__name__}({self._numerator}, {self._denominator})"

    def __str__(self):
        if self._denominator == 1:
            return str(self._numerator)
        return f"{self._numerator}/{self._denominator}"

    def __bool__(self):
        return self._numerator != 0

    def __float__(self):
        return self._numerator / self._denominator

    def __int__(self):
        return self.__trunc__()

    def __trunc__(self):
        if self._numerator < 0:
            return -((-self._numerator) // self._denominator)
        return self._numerator // self._denominator

    def __floor__(self):
        return self._numerator // self._denominator

    def __ceil__(self):
        return -((-self._numerator) // self._denominator)

    def __round__(self, ndigits=None):
        if ndigits is None:
            floor, remainder = divmod(self._numerator, self._denominator)
            if remainder * 2 < self._denominator:
                return floor
            if remainder * 2 > self._denominator:
                return floor + 1
            if floor % 2 == 0:
                return floor
            return floor + 1
        shift = 10 ** abs(ndigits)
        if ndigits > 0:
            return Fraction(round(self * shift), shift)
        return Fraction(round(self / shift) * shift)

    def __pos__(self):
        return Fraction(self._numerator, self._denominator)

    def __neg__(self):
        return Fraction(-self._numerator, self._denominator)

    def __abs__(self):
        return Fraction(abs(self._numerator), self._denominator)

    def _coerce(self, other):
        if isinstance(other, Fraction):
            return other._numerator, other._denominator
        if isinstance(other, int):
            return int(other), 1
        if isinstance(other, numbers.Rational):
            return int(other.numerator), int(other.denominator)
        return None

    def __add__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(self._numerator * od + on * self._denominator, self._denominator * od)
        if isinstance(other, float):
            return float(self) + other
        if isinstance(other, complex):
            return complex(self) + other
        return NotImplemented

    def __radd__(self, other):
        return self.__add__(other)

    def __sub__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(self._numerator * od - on * self._denominator, self._denominator * od)
        if isinstance(other, float):
            return float(self) - other
        if isinstance(other, complex):
            return complex(self) - other
        return NotImplemented

    def __rsub__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(on * self._denominator - self._numerator * od, self._denominator * od)
        if isinstance(other, float):
            return other - float(self)
        if isinstance(other, complex):
            return other - complex(self)
        return NotImplemented

    def __mul__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(self._numerator * on, self._denominator * od)
        if isinstance(other, float):
            return float(self) * other
        if isinstance(other, complex):
            return complex(self) * other
        return NotImplemented

    def __rmul__(self, other):
        return self.__mul__(other)

    def __truediv__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(self._numerator * od, self._denominator * on)
        if isinstance(other, float):
            return float(self) / other
        if isinstance(other, complex):
            return complex(self) / other
        return NotImplemented

    def __rtruediv__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            return Fraction(on * self._denominator, od * self._numerator)
        if isinstance(other, float):
            return other / float(self)
        if isinstance(other, complex):
            return other / complex(self)
        return NotImplemented

    def __floordiv__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if on == 0:
                raise ZeroDivisionError("Fraction division by zero")
            return (self._numerator * od) // (self._denominator * on)
        if isinstance(other, float):
            return float(self) // other
        return NotImplemented

    def __rfloordiv__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if self._numerator == 0:
                raise ZeroDivisionError("Fraction division by zero")
            return (on * self._denominator) // (od * self._numerator)
        if isinstance(other, float):
            return other // float(self)
        return NotImplemented

    def __mod__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if on == 0:
                raise ZeroDivisionError("Fraction modulo by zero")
            div = (self._numerator * od) // (self._denominator * on)
            return Fraction(self._numerator * od - div * self._denominator * on, self._denominator * od)
        if isinstance(other, float):
            return float(self) % other
        return NotImplemented

    def __rmod__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if self._numerator == 0:
                raise ZeroDivisionError("Fraction modulo by zero")
            div = (on * self._denominator) // (od * self._numerator)
            return Fraction(on * self._denominator - div * od * self._numerator, od * self._denominator)
        if isinstance(other, float):
            return other % float(self)
        return NotImplemented

    def __divmod__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if on == 0:
                raise ZeroDivisionError("Fraction division by zero")
            q, r = divmod(self._numerator * od, self._denominator * on)
            return (q, Fraction(r, self._denominator * od))
        if isinstance(other, float):
            return divmod(float(self), other)
        return NotImplemented

    def __rdivmod__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if self._numerator == 0:
                raise ZeroDivisionError("Fraction division by zero")
            q, r = divmod(on * self._denominator, od * self._numerator)
            return (q, Fraction(r, od * self._denominator))
        if isinstance(other, float):
            return divmod(other, float(self))
        return NotImplemented

    def __pow__(self, other):
        pair = self._coerce(other)
        if pair is not None:
            on, od = pair
            if od == 1:
                if on >= 0:
                    return Fraction(self._numerator ** on, self._denominator ** on)
                if self._numerator >= 0:
                    return Fraction(self._denominator ** (-on), self._numerator ** (-on))
                return Fraction((-self._denominator) ** (-on), (-self._numerator) ** (-on))
            return float(self) ** (on / od)
        if isinstance(other, (float, complex)):
            return float(self) ** other
        return NotImplemented

    def __rpow__(self, other):
        if self._denominator == 1 and isinstance(other, int):
            return Fraction(other) ** self._numerator
        if isinstance(other, (int, float, complex)):
            return other ** float(self)
        return NotImplemented

    def __eq__(self, other):
        if isinstance(other, int):
            return self._denominator == 1 and self._numerator == int(other)
        if isinstance(other, Fraction):
            return self._numerator == other._numerator and self._denominator == other._denominator
        if isinstance(other, numbers.Rational):
            return self._numerator == other.numerator and self._denominator == other.denominator
        if isinstance(other, complex):
            if other.imag != 0:
                return False
            other = other.real
        if isinstance(other, float):
            if math.isnan(other) or math.isinf(other):
                return False
            on, od = _float_ratio(other)
            return self._numerator * od == on * self._denominator
        return NotImplemented

    def _cmp(self, other):
        if isinstance(other, int):
            a, b = self._numerator, int(other) * self._denominator
            return -1 if a < b else (1 if a > b else 0)
        if isinstance(other, Fraction):
            a = self._numerator * other._denominator
            b = other._numerator * self._denominator
            return -1 if a < b else (1 if a > b else 0)
        if isinstance(other, numbers.Rational):
            a = self._numerator * int(other.denominator)
            b = int(other.numerator) * self._denominator
            return -1 if a < b else (1 if a > b else 0)
        if isinstance(other, float):
            if math.isnan(other):
                return None
            if math.isinf(other):
                return -1 if other > 0 else 1
            on, od = _float_ratio(other)
            a, b = self._numerator * od, on * self._denominator
            return -1 if a < b else (1 if a > b else 0)
        return NotImplemented

    def __lt__(self, other):
        c = self._cmp(other)
        if c is NotImplemented:
            return NotImplemented
        return c is not None and c < 0

    def __le__(self, other):
        c = self._cmp(other)
        if c is NotImplemented:
            return NotImplemented
        return c is not None and c <= 0

    def __gt__(self, other):
        c = self._cmp(other)
        if c is NotImplemented:
            return NotImplemented
        return c is not None and c > 0

    def __ge__(self, other):
        c = self._cmp(other)
        if c is NotImplemented:
            return NotImplemented
        return c is not None and c >= 0

    def __hash__(self):
        if self._denominator % _PyHASH_MODULUS == 0:
            hash_ = _PyHASH_INF
        else:
            dinv = pow(self._denominator, -1, _PyHASH_MODULUS)
            hash_ = (abs(self._numerator) * dinv) % _PyHASH_MODULUS
        result = hash_ if self._numerator >= 0 else -hash_
        return -2 if result == -1 else result

    def __format__(self, format_spec):
        if not format_spec:
            return str(self)
        return format(float(self), format_spec)
