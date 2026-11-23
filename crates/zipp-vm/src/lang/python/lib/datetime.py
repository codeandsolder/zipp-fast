"""Date, time, datetime, timedelta, and timezone support for Zipp."""
import math
import re
import time as _time

MINYEAR = 1
MAXYEAR = 9999

_US_PER_SEC = 1_000_000
_US_PER_MIN = 60 * _US_PER_SEC
_US_PER_HOUR = 3600 * _US_PER_SEC
_US_PER_DAY = 86400 * _US_PER_SEC
_EPOCH_ORD = 719163

_MONTH_LEN = (0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)
_MONTH_ACC = (0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334)
_WD_SHORT = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
_WD_LONG = ("Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday")
_MON_SHORT = ("", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
_MON_LONG = (
    "", "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
)


def _leap(y):
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)


def _dim(y, m):
    if m == 2 and _leap(y):
        return 29
    return _MONTH_LEN[m]


def _days_before_year(y):
    k = y - 1
    return k * 365 + k // 4 - k // 100 + k // 400


def _ymd_to_ord(y, m, d):
    acc = _MONTH_ACC[m] + (1 if m > 2 and _leap(y) else 0)
    return _days_before_year(y) + acc + d


def _ord_to_ymd(n):
    if not (1 <= n <= 3652059):
        raise ValueError("ordinal must be >= 1 and <= 3652059")
    n -= 1
    c400, n = divmod(n, 146097)
    c100, n = divmod(n, 36524)
    c4, n = divmod(n, 1461)
    c1, n = divmod(n, 365)
    y = c400 * 400 + c100 * 100 + c4 * 4 + c1 + 1
    if c100 == 4 or c1 == 4:
        return y - 1, 12, 31
    leap = _leap(y)
    m = (n + 50) >> 5
    before = _MONTH_ACC[m] + (1 if m > 2 and leap else 0)
    if before > n:
        m -= 1
        before = _MONTH_ACC[m] + (1 if m > 2 and leap else 0)
    return y, m, n - before + 1


def _need_int(val, name):
    if isinstance(val, bool) or not isinstance(val, int):
        if hasattr(val, "__index__"):
            return val.__index__()
        raise TypeError("%s must be int, not %s" % (name, type(val).__name__))
    return int(val)


def _check_ymd(y, m, d):
    y = _need_int(y, "year")
    m = _need_int(m, "month")
    d = _need_int(d, "day")
    if not (MINYEAR <= y <= MAXYEAR):
        raise ValueError("year %d is out of range" % y)
    if not (1 <= m <= 12):
        raise ValueError("month must be in 1..12")
    mx = _dim(y, m)
    if not (1 <= d <= mx):
        raise ValueError("day is out of range for month")
    return y, m, d


def _check_hmsu(h, mi, s, u, fold):
    h = _need_int(h, "hour")
    mi = _need_int(mi, "minute")
    s = _need_int(s, "second")
    u = _need_int(u, "microsecond")
    fold = _need_int(fold, "fold")
    if not (0 <= h <= 23):
        raise ValueError("hour must be in 0..23")
    if not (0 <= mi <= 59):
        raise ValueError("minute must be in 0..59")
    if not (0 <= s <= 59):
        raise ValueError("second must be in 0..59")
    if not (0 <= u <= 999999):
        raise ValueError("microsecond must be in 0..999999")
    if fold not in (0, 1):
        raise ValueError("fold must be either 0 or 1")
    return h, mi, s, u, fold


def _check_tz(tz):
    if tz is not None and not isinstance(tz, tzinfo):
        raise TypeError("tzinfo argument must be None or of a tzinfo subclass")


def _fmt_offset(off, colon=True):
    if off is None:
        return ""
    total_us = off._us
    sign = "+" if total_us >= 0 else "-"
    if total_us < 0:
        total_us = -total_us
    total_s, us = divmod(total_us, _US_PER_SEC)
    hh, rem = divmod(total_s, 3600)
    mm, ss = divmod(rem, 60)
    sep = ":" if colon else ""
    out = "%s%02d%s%02d" % (sign, hh, sep, mm)
    if ss or us:
        out += "%s%02d" % (sep, ss)
        if us:
            out += ".%06d" % us
    return out


def _fmt_time_part(h, mi, s, u, timespec):
    if timespec == "auto":
        timespec = "microseconds" if u else "seconds"
    if timespec == "hours":
        return "%02d" % h
    if timespec == "minutes":
        return "%02d:%02d" % (h, mi)
    if timespec == "seconds":
        return "%02d:%02d:%02d" % (h, mi, s)
    if timespec == "milliseconds":
        return "%02d:%02d:%02d.%03d" % (h, mi, s, u // 1000)
    if timespec == "microseconds":
        return "%02d:%02d:%02d.%06d" % (h, mi, s, u)
    raise ValueError("Unknown timespec value")


def _parse_frac_us(digits):
    if not digits:
        return 0
    if len(digits) <= 6:
        return int(digits.ljust(6, "0"))
    return int(digits[:6])


def _parse_iso_tz(s):
    if not s:
        return None
    if s in ("Z", "z"):
        return timezone.utc
    sign = 1 if s[0] == "+" else -1 if s[0] == "-" else 0
    if sign == 0:
        raise ValueError("Invalid isoformat offset: %r" % s)
    body = s[1:]
    m = re.match(r"^(\d{2})(?::?(\d{2})(?::?(\d{2})(?:\.(\d+))?)?)?$", body)
    if not m:
        raise ValueError("Invalid isoformat offset: %r" % s)
    hh = int(m.group(1))
    mm = int(m.group(2)) if m.group(2) is not None else 0
    ss = int(m.group(3)) if m.group(3) is not None else 0
    us = _parse_frac_us(m.group(4) or "")
    if hh >= 24 or mm >= 60 or ss >= 60:
        raise ValueError("Invalid isoformat offset: %r" % s)
    off = timedelta(hours=hh, minutes=mm, seconds=ss, microseconds=us)
    return timezone(off if sign > 0 else -off)


def _iso_week1_monday(year):
    jan4 = _ymd_to_ord(year, 1, 4)
    return jan4 - ((jan4 - 1) % 7)


def _parse_iso_date_str(s):
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})$", s)
    if m:
        return int(m.group(1)), int(m.group(2)), int(m.group(3))
    m = re.match(r"^(\d{4})(\d{2})(\d{2})$", s)
    if m:
        return int(m.group(1)), int(m.group(2)), int(m.group(3))
    m = re.match(r"^(\d{4})-?W(\d{2})(?:-?([1-7]))?$", s)
    if m:
        yr = int(m.group(1))
        wk = int(m.group(2))
        wd = int(m.group(3)) if m.group(3) is not None else 1
        d = date.fromisocalendar(yr, wk, wd)
        return d.year, d.month, d.day
    raise ValueError("Invalid isoformat string: %r" % s)


def _parse_iso_time_str(s):
    tz = None
    m_tz = re.search(r"([Zz]|[+-]\d{2}(?::?\d{2}(?::?\d{2}(?:\.\d+)?)?)?)$", s)
    if m_tz:
        tz = _parse_iso_tz(m_tz.group(1))
        s = s[:m_tz.start()]
    m = re.match(r"^(\d{2})(?::(\d{2})(?::(\d{2})(?:[.,](\d+))?)?)?$", s)
    if not m:
        m = re.match(r"^(\d{2})(\d{2})(?:(\d{2})(?:[.,](\d+))?)?$", s)
    if not m:
        raise ValueError("Invalid isoformat string: %r" % s)
    hh = int(m.group(1))
    mm = int(m.group(2)) if m.group(2) is not None else 0
    ss = int(m.group(3)) if m.group(3) is not None else 0
    us = _parse_frac_us(m.group(4) or "")
    return hh, mm, ss, us, tz


def _strftime(obj, fmt):
    is_dt = isinstance(obj, datetime)
    is_d = isinstance(obj, date)
    y = obj.year if is_d else 1900
    mo = obj.month if is_d else 1
    da = obj.day if is_d else 1
    hh = obj.hour if (is_dt or isinstance(obj, time)) else 0
    mi = obj.minute if (is_dt or isinstance(obj, time)) else 0
    ss = obj.second if (is_dt or isinstance(obj, time)) else 0
    us = obj.microsecond if (is_dt or isinstance(obj, time)) else 0
    wd = obj.weekday() if is_d else 0
    yday = (_ymd_to_ord(y, mo, da) - _ymd_to_ord(y, 1, 1) + 1) if is_d else 1
    out = []
    i = 0
    n = len(fmt)
    while i < n:
        c = fmt[i]
        if c != "%" or i + 1 >= n:
            out.append(c)
            i += 1
            continue
        code = fmt[i + 1]
        i += 2
        if code == "%":
            out.append("%")
        elif code == "Y":
            out.append("%04d" % y)
        elif code == "y":
            out.append("%02d" % (y % 100))
        elif code == "m":
            out.append("%02d" % mo)
        elif code == "d":
            out.append("%02d" % da)
        elif code == "H":
            out.append("%02d" % hh)
        elif code == "I":
            h12 = hh % 12
            out.append("%02d" % (12 if h12 == 0 else h12))
        elif code == "M":
            out.append("%02d" % mi)
        elif code == "S":
            out.append("%02d" % ss)
        elif code == "f":
            out.append("%06d" % us)
        elif code == "p":
            out.append("AM" if hh < 12 else "PM")
        elif code == "a":
            out.append(_WD_SHORT[wd])
        elif code == "A":
            out.append(_WD_LONG[wd])
        elif code == "w":
            out.append(str((wd + 1) % 7))
        elif code == "u":
            out.append(str(wd + 1))
        elif code == "b" or code == "h":
            out.append(_MON_SHORT[mo])
        elif code == "B":
            out.append(_MON_LONG[mo])
        elif code == "j":
            out.append("%03d" % yday)
        elif code == "z":
            off = obj.utcoffset() if (is_dt or isinstance(obj, time)) else None
            out.append(_fmt_offset(off, colon=False))
        elif code == "Z":
            tn = obj.tzname() if (is_dt or isinstance(obj, time)) else None
            out.append(tn or "")
        elif code == "F":
            out.append("%04d-%02d-%02d" % (y, mo, da))
        elif code == "T":
            out.append("%02d:%02d:%02d" % (hh, mi, ss))
        elif code == "G":
            out.append("%04d" % (obj.isocalendar()[0] if is_d else 1900))
        elif code == "V":
            out.append("%02d" % (obj.isocalendar()[1] if is_d else 1))
        elif code == "c":
            out.append("%s %s %2d %02d:%02d:%02d %04d" % (_WD_SHORT[wd], _MON_SHORT[mo], da, hh, mi, ss, y))
        elif code == "x":
            out.append("%02d/%02d/%02d" % (mo, da, y % 100))
        elif code == "X":
            out.append("%02d:%02d:%02d" % (hh, mi, ss))
        else:
            out.append("%" + code)
    return "".join(out)


class timedelta:
    __slots__ = ("_days", "_seconds", "_microseconds", "_us")

    def __init__(self, days=0, seconds=0, microseconds=0, milliseconds=0, minutes=0, hours=0, weeks=0):
        total = (
            (weeks * 7 + days) * _US_PER_DAY
            + hours * _US_PER_HOUR
            + minutes * _US_PER_MIN
            + seconds * _US_PER_SEC
            + milliseconds * 1000
            + microseconds
        )
        if isinstance(total, float):
            if math.isnan(total) or math.isinf(total):
                raise OverflowError("cannot convert float infinity or NaN to integer")
            total = int(round(total))
        else:
            total = int(total)
        d, rem = divmod(total, _US_PER_DAY)
        s, u = divmod(rem, _US_PER_SEC)
        if abs(d) > 999999999:
            raise OverflowError("timedelta days out of range: %d" % d)
        self._days = d
        self._seconds = s
        self._microseconds = u
        self._us = total

    @classmethod
    def _from_us(cls, us):
        obj = cls.__new__(cls)
        d, rem = divmod(us, _US_PER_DAY)
        s, u = divmod(rem, _US_PER_SEC)
        if abs(d) > 999999999:
            raise OverflowError("timedelta days out of range")
        obj._days = d
        obj._seconds = s
        obj._microseconds = u
        obj._us = us
        return obj

    @property
    def days(self):
        return self._days

    @property
    def seconds(self):
        return self._seconds

    @property
    def microseconds(self):
        return self._microseconds

    def total_seconds(self):
        return self._us / _US_PER_SEC

    def __repr__(self):
        args = []
        if self._days:
            args.append("days=%d" % self._days)
        if self._seconds:
            args.append("seconds=%d" % self._seconds)
        if self._microseconds:
            args.append("microseconds=%d" % self._microseconds)
        if not args:
            args.append("0")
        return "datetime.timedelta(%s)" % ", ".join(args)

    def __str__(self):
        mm, ss = divmod(self._seconds, 60)
        hh, mm = divmod(mm, 60)
        s = "%d:%02d:%02d" % (hh, mm, ss)
        if self._days:
            plural = "s" if abs(self._days) != 1 else ""
            s = "%d day%s, %s" % (self._days, plural, s)
        if self._microseconds:
            s = "%s.%06d" % (s, self._microseconds)
        return s

    def __bool__(self):
        return self._us != 0

    def __hash__(self):
        return hash((self._days, self._seconds, self._microseconds))

    def __eq__(self, other):
        if isinstance(other, timedelta):
            return self._us == other._us
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, timedelta):
            return self._us < other._us
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, timedelta):
            return self._us <= other._us
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, timedelta):
            return self._us > other._us
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, timedelta):
            return self._us >= other._us
        return NotImplemented

    def __add__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(self._us + other._us)
        return NotImplemented

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(self._us - other._us)
        return NotImplemented

    def __neg__(self):
        return timedelta._from_us(-self._us)

    def __pos__(self):
        return self

    def __abs__(self):
        return -self if self._us < 0 else self

    def __mul__(self, other):
        if isinstance(other, int) and not isinstance(other, bool):
            return timedelta._from_us(self._us * other)
        if isinstance(other, float):
            return timedelta._from_us(int(round(self._us * other)))
        return NotImplemented

    __rmul__ = __mul__

    def __truediv__(self, other):
        if isinstance(other, timedelta):
            return self._us / other._us
        if isinstance(other, (int, float)) and not isinstance(other, bool):
            return timedelta._from_us(int(round(self._us / other)))
        return NotImplemented

    def __floordiv__(self, other):
        if isinstance(other, timedelta):
            return self._us // other._us
        if isinstance(other, int) and not isinstance(other, bool):
            return timedelta._from_us(self._us // other)
        return NotImplemented

    def __mod__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(self._us % other._us)
        return NotImplemented

    def __divmod__(self, other):
        if isinstance(other, timedelta):
            q, r = divmod(self._us, other._us)
            return q, timedelta._from_us(r)
        return NotImplemented


timedelta.min = timedelta(-999999999)
timedelta.max = timedelta(days=999999999, hours=23, minutes=59, seconds=59, microseconds=999999)
timedelta.resolution = timedelta(microseconds=1)


class tzinfo:
    def tzname(self, dt):
        raise NotImplementedError

    def utcoffset(self, dt):
        raise NotImplementedError

    def dst(self, dt):
        raise NotImplementedError

    def fromutc(self, dt):
        if not isinstance(dt, datetime):
            raise TypeError("fromutc() requires a datetime argument")
        if dt.tzinfo is not self:
            raise ValueError("dt.tzinfo is not self")
        off = dt.utcoffset()
        if off is None:
            raise ValueError("fromutc() requires a non-None utcoffset()")
        dst = dt.dst()
        if dst is None:
            raise ValueError("fromutc() requires a non-None dst()")
        delta = off - dst
        if delta:
            dt = dt + delta
            dst = dt.dst()
            if dst is None:
                raise ValueError("fromutc(): dt.dst() gave inconsistent results")
        return dt + dst


class timezone(tzinfo):
    __slots__ = ("_offset", "_name")

    def __init__(self, offset, name=None):
        if not isinstance(offset, timedelta):
            raise TypeError("offset must be a timedelta")
        if offset <= timedelta(hours=-24) or offset >= timedelta(hours=24):
            raise ValueError("offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24)")
        if name is not None and not isinstance(name, str):
            raise TypeError("name must be a string")
        self._offset = offset
        self._name = name

    def utcoffset(self, dt):
        if isinstance(dt, datetime) or dt is None:
            return self._offset
        raise TypeError("utcoffset() argument must be a datetime instance or None")

    def dst(self, dt):
        if isinstance(dt, datetime) or dt is None:
            return None
        raise TypeError("dst() argument must be a datetime instance or None")

    def tzname(self, dt):
        if isinstance(dt, datetime) or dt is None:
            if self._name is not None:
                return self._name
            if self._offset._us == 0:
                return "UTC"
            return "UTC" + _fmt_offset(self._offset, colon=True)
        raise TypeError("tzname() argument must be a datetime instance or None")

    def fromutc(self, dt):
        if isinstance(dt, datetime):
            if dt.tzinfo is not self:
                raise ValueError("fromutc: dt.tzinfo is not self")
            return dt + self._offset
        raise TypeError("fromutc() argument must be a datetime instance")

    def __eq__(self, other):
        if isinstance(other, timezone):
            return self._offset == other._offset
        return NotImplemented

    def __hash__(self):
        return hash(self._offset)

    def __repr__(self):
        if self is timezone.utc:
            return "datetime.timezone.utc"
        if self._name is None:
            return "datetime.timezone(%r)" % (self._offset,)
        return "datetime.timezone(%r, %r)" % (self._offset, self._name)

    def __str__(self):
        return self.tzname(None)


timezone.utc = timezone(timedelta(0))
timezone.min = timezone(timedelta(hours=-23, minutes=-59))
timezone.max = timezone(timedelta(hours=23, minutes=59))
UTC = timezone.utc


class date:
    __slots__ = ("_year", "_month", "_day")

    def __init__(self, year, month, day):
        y, m, d = _check_ymd(year, month, day)
        self._year = y
        self._month = m
        self._day = d

    @property
    def year(self):
        return self._year

    @property
    def month(self):
        return self._month

    @property
    def day(self):
        return self._day

    @classmethod
    def fromtimestamp(cls, ts):
        dt = datetime.fromtimestamp(ts)
        return cls(dt.year, dt.month, dt.day)

    @classmethod
    def today(cls):
        return cls.fromtimestamp(_time.time())

    @classmethod
    def fromordinal(cls, n):
        y, m, d = _ord_to_ymd(_need_int(n, "ordinal"))
        return cls(y, m, d)

    @classmethod
    def fromisoformat(cls, date_string):
        if not isinstance(date_string, str):
            raise TypeError("fromisoformat: argument must be str")
        y, m, d = _parse_iso_date_str(date_string)
        return cls(y, m, d)

    @classmethod
    def fromisocalendar(cls, year, week, day):
        year = _need_int(year, "year")
        week = _need_int(week, "week")
        day = _need_int(day, "day")
        if not (MINYEAR <= year <= MAXYEAR):
            raise ValueError("Year is out of range: %d" % year)
        if not (1 <= day <= 7):
            raise ValueError("Invalid day: %d (range is [1, 7])" % day)
        first = _iso_week1_monday(year)
        nxt = _iso_week1_monday(year + 1)
        max_weeks = (nxt - first) // 7
        if not (1 <= week <= max_weeks):
            raise ValueError("Invalid week: %d" % week)
        return cls.fromordinal(first + (week - 1) * 7 + (day - 1))

    def toordinal(self):
        return _ymd_to_ord(self._year, self._month, self._day)

    def weekday(self):
        return (self.toordinal() + 6) % 7

    def isoweekday(self):
        return self.weekday() + 1

    def isocalendar(self):
        y = self._year
        ord_d = self.toordinal()
        w1 = _iso_week1_monday(y)
        if ord_d < w1:
            y -= 1
            w1 = _iso_week1_monday(y)
        else:
            nxt = _iso_week1_monday(y + 1)
            if ord_d >= nxt:
                y += 1
                w1 = nxt
        week, wd = divmod(ord_d - w1, 7)
        return (y, week + 1, wd + 1)

    def replace(self, year=None, month=None, day=None):
        return type(self)(
            self._year if year is None else year,
            self._month if month is None else month,
            self._day if day is None else day,
        )

    def isoformat(self):
        return "%04d-%02d-%02d" % (self._year, self._month, self._day)

    __str__ = isoformat

    def ctime(self):
        return "%s %s %2d 00:00:00 %04d" % (_WD_SHORT[self.weekday()], _MON_SHORT[self._month], self._day, self._year)

    def strftime(self, fmt):
        return _strftime(self, fmt)

    def __format__(self, fmt):
        if not fmt:
            return str(self)
        return self.strftime(fmt)

    def timetuple(self):
        yday = self.toordinal() - _ymd_to_ord(self._year, 1, 1) + 1
        return (self._year, self._month, self._day, 0, 0, 0, self.weekday(), yday, -1)

    def __repr__(self):
        return "datetime.date(%d, %d, %d)" % (self._year, self._month, self._day)

    def __hash__(self):
        return hash((self._year, self._month, self._day))

    def __eq__(self, other):
        if type(other) is date or (isinstance(other, date) and not isinstance(other, datetime)):
            return (self._year, self._month, self._day) == (other._year, other._month, other._day)
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return (self._year, self._month, self._day) < (other._year, other._month, other._day)
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return (self._year, self._month, self._day) <= (other._year, other._month, other._day)
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return (self._year, self._month, self._day) > (other._year, other._month, other._day)
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return (self._year, self._month, self._day) >= (other._year, other._month, other._day)
        return NotImplemented

    def __add__(self, other):
        if isinstance(other, timedelta):
            o = self.toordinal() + other._days
            if not (1 <= o <= 3652059):
                raise OverflowError("date value out of range")
            return date.fromordinal(o)
        return NotImplemented

    __radd__ = __add__

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return self + (-other)
        if isinstance(other, date) and not isinstance(other, datetime):
            return timedelta(days=self.toordinal() - other.toordinal())
        return NotImplemented


date.min = date(MINYEAR, 1, 1)
date.max = date(MAXYEAR, 12, 31)
date.resolution = timedelta(days=1)


class time:
    __slots__ = ("_hour", "_minute", "_second", "_microsecond", "_tzinfo", "_fold")

    def __init__(self, hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0):
        _check_tz(tzinfo)
        h, mi, s, u, f = _check_hmsu(hour, minute, second, microsecond, fold)
        self._hour = h
        self._minute = mi
        self._second = s
        self._microsecond = u
        self._tzinfo = tzinfo
        self._fold = f

    @property
    def hour(self):
        return self._hour

    @property
    def minute(self):
        return self._minute

    @property
    def second(self):
        return self._second

    @property
    def microsecond(self):
        return self._microsecond

    @property
    def tzinfo(self):
        return self._tzinfo

    @property
    def fold(self):
        return self._fold

    def utcoffset(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.utcoffset(None)

    def dst(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.dst(None)

    def tzname(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.tzname(None)

    def _day_us(self):
        return ((self._hour * 60 + self._minute) * 60 + self._second) * _US_PER_SEC + self._microsecond

    @classmethod
    def fromisoformat(cls, time_string):
        if not isinstance(time_string, str):
            raise TypeError("fromisoformat: argument must be str")
        h, mi, s, u, tz = _parse_iso_time_str(time_string)
        return cls(h, mi, s, u, tz)

    def replace(self, hour=None, minute=None, second=None, microsecond=None, tzinfo=True, *, fold=None):
        return type(self)(
            self._hour if hour is None else hour,
            self._minute if minute is None else minute,
            self._second if second is None else second,
            self._microsecond if microsecond is None else microsecond,
            self._tzinfo if tzinfo is True else tzinfo,
            fold=self._fold if fold is None else fold,
        )

    def isoformat(self, timespec="auto"):
        s = _fmt_time_part(self._hour, self._minute, self._second, self._microsecond, timespec)
        return s + _fmt_offset(self.utcoffset(), colon=True)

    __str__ = isoformat

    def strftime(self, fmt):
        return _strftime(self, fmt)

    def __format__(self, fmt):
        if not fmt:
            return str(self)
        return self.strftime(fmt)

    def __repr__(self):
        parts = [str(self._hour), str(self._minute)]
        if self._second or self._microsecond:
            parts.append(str(self._second))
        if self._microsecond:
            parts.append(str(self._microsecond))
        if self._tzinfo is not None:
            parts.append("tzinfo=%r" % (self._tzinfo,))
        if self._fold:
            parts.append("fold=%d" % self._fold)
        return "datetime.time(%s)" % ", ".join(parts)

    def _cmp_key(self, other, eq_only=False):
        o1 = self.utcoffset()
        o2 = other.utcoffset()
        if (o1 is None) != (o2 is None):
            if eq_only:
                return None, None
            raise TypeError("can't compare offset-naive and offset-aware times")
        if o1 is None:
            return self._day_us(), other._day_us()
        return self._day_us() - o1._us, other._day_us() - o2._us

    def __eq__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, eq_only=True)
        return False if a is None else a == b

    def __lt__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other)
        return a < b

    def __le__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other)
        return a <= b

    def __gt__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other)
        return a > b

    def __ge__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other)
        return a >= b

    def __hash__(self):
        off = self.utcoffset()
        if off is None:
            return hash((self._hour, self._minute, self._second, self._microsecond))
        return hash(self._day_us() - off._us)


time.min = time(0, 0, 0, 0)
time.max = time(23, 59, 59, 999999)
time.resolution = timedelta(microseconds=1)


class datetime(date):
    __slots__ = ("_hour", "_minute", "_second", "_microsecond", "_tzinfo", "_fold")

    def __init__(self, year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0):
        super().__init__(year, month, day)
        _check_tz(tzinfo)
        h, mi, s, u, f = _check_hmsu(hour, minute, second, microsecond, fold)
        self._hour = h
        self._minute = mi
        self._second = s
        self._microsecond = u
        self._tzinfo = tzinfo
        self._fold = f

    @property
    def hour(self):
        return self._hour

    @property
    def minute(self):
        return self._minute

    @property
    def second(self):
        return self._second

    @property
    def microsecond(self):
        return self._microsecond

    @property
    def tzinfo(self):
        return self._tzinfo

    @property
    def fold(self):
        return self._fold

    def _local_us(self):
        days = self.toordinal() - 1
        secs = (self._hour * 60 + self._minute) * 60 + self._second
        return days * _US_PER_DAY + secs * _US_PER_SEC + self._microsecond

    @classmethod
    def _from_local_us(cls, total_us, tz=None, fold=0):
        days, rem = divmod(total_us, _US_PER_DAY)
        secs, us = divmod(rem, _US_PER_SEC)
        hh, rem_s = divmod(secs, 3600)
        mm, ss = divmod(rem_s, 60)
        y, mo, d = _ord_to_ymd(days + 1)
        return cls(y, mo, d, hh, mm, ss, us, tz, fold=fold)

    @classmethod
    def fromtimestamp(cls, ts, tz=None):
        _check_tz(tz)
        us = int(round(ts * _US_PER_SEC))
        base_us = (_EPOCH_ORD - 1) * _US_PER_DAY + us
        if tz is None:
            return cls._from_local_us(base_us, None)
        utc_dt = cls._from_local_us(base_us, tz)
        return tz.fromutc(utc_dt)

    @classmethod
    def utcfromtimestamp(cls, ts):
        us = int(round(ts * _US_PER_SEC))
        return cls._from_local_us((_EPOCH_ORD - 1) * _US_PER_DAY + us, None)

    @classmethod
    def now(cls, tz=None):
        return cls.fromtimestamp(_time.time(), tz)

    @classmethod
    def today(cls):
        return cls.fromtimestamp(_time.time(), None)

    @classmethod
    def utcnow(cls):
        return cls.utcfromtimestamp(_time.time())

    @classmethod
    def fromordinal(cls, n):
        y, m, d = _ord_to_ymd(_need_int(n, "ordinal"))
        return cls(y, m, d)

    @classmethod
    def combine(cls, d, t, tzinfo=True):
        if not isinstance(d, date):
            raise TypeError("date argument must be a date instance")
        if not isinstance(t, time):
            raise TypeError("time argument must be a time instance")
        tz = t.tzinfo if tzinfo is True else tzinfo
        return cls(d.year, d.month, d.day, t.hour, t.minute, t.second, t.microsecond, tz, fold=t.fold)

    @classmethod
    def fromisoformat(cls, date_string):
        if not isinstance(date_string, str):
            raise TypeError("fromisoformat: argument must be str")
        s = date_string
        # Date-only string is accepted by Python 3.11+ datetime.fromisoformat.
        if len(s) <= 10 and ("T" not in s and "t" not in s and " " not in s):
            y, m, d = _parse_iso_date_str(s)
            return cls(y, m, d)
        # Split date prefix and time suffix separated by any single separator char.
        m_date = re.match(r"^(\d{4}-\d{2}-\d{2}|\d{8}|\d{4}-?W\d{2}-?[1-7])", s)
        if not m_date:
            raise ValueError("Invalid isoformat string: %r" % date_string)
        dpart = m_date.group(1)
        rest = s[m_date.end():]
        y, mo, da = _parse_iso_date_str(dpart)
        if not rest:
            return cls(y, mo, da)
        if len(rest) < 2:
            raise ValueError("Invalid isoformat string: %r" % date_string)
        tpart = rest[1:]
        hh, mi, ss, us, tz = _parse_iso_time_str(tpart)
        return cls(y, mo, da, hh, mi, ss, us, tz)

    @classmethod
    def strptime(cls, date_string, fmt):
        tokens = []
        i = 0
        n = len(fmt)
        while i < n:
            if fmt[i] == "%" and i + 1 < n:
                c = fmt[i + 1]
                i += 2
                if c == "%":
                    tokens.append("%")
                elif c == "Y":
                    tokens.append(r"(?P<Y>\d{4})")
                elif c == "y":
                    tokens.append(r"(?P<y>\d{2})")
                elif c == "m":
                    tokens.append(r"(?P<m>\d{1,2})")
                elif c == "d":
                    tokens.append(r"(?P<d>\d{1,2})")
                elif c == "H":
                    tokens.append(r"(?P<H>\d{1,2})")
                elif c == "I":
                    tokens.append(r"(?P<I>\d{1,2})")
                elif c == "M":
                    tokens.append(r"(?P<M>\d{1,2})")
                elif c == "S":
                    tokens.append(r"(?P<S>\d{1,2})")
                elif c == "f":
                    tokens.append(r"(?P<f>\d{1,6})")
                elif c == "p":
                    tokens.append(r"(?P<p>AM|PM|am|pm)")
                elif c == "b":
                    tokens.append(r"(?P<b>Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)")
                elif c == "B":
                    tokens.append(r"(?P<B>January|February|March|April|May|June|July|August|September|October|November|December)")
                elif c == "z":
                    tokens.append(r"(?P<z>[Zz]|[+-]\d{2}:?\d{2}(?::?\d{2}(?:\.\d+)?)?)")
                elif c == "Z":
                    tokens.append(r"(?P<Z>[A-Za-z_/+0-9-]+)")
                elif c == "j":
                    tokens.append(r"(?P<j>\d{1,3})")
                else:
                    tokens.append(re.escape("%" + c))
            elif fmt[i].isspace():
                while i < n and fmt[i].isspace():
                    i += 1
                tokens.append(r"\s+")
            else:
                tokens.append(re.escape(fmt[i]))
                i += 1
        m = re.fullmatch("".join(tokens), date_string)
        if not m:
            raise ValueError("time data %r does not match format %r" % (date_string, fmt))
        gd = m.groupdict()
        year = 1900
        if gd.get("Y"):
            year = int(gd["Y"])
        elif gd.get("y"):
            yy = int(gd["y"])
            year = 1900 + yy if yy >= 69 else 2000 + yy
        month = 1
        if gd.get("m"):
            month = int(gd["m"])
        elif gd.get("b"):
            month = _MON_SHORT.index(gd["b"])
        elif gd.get("B"):
            month = _MON_LONG.index(gd["B"])
        day = int(gd["d"]) if gd.get("d") else 1
        if gd.get("j") and not gd.get("m") and not gd.get("d"):
            d_obj = date.fromordinal(_ymd_to_ord(year, 1, 1) + int(gd["j"]) - 1)
            month, day = d_obj.month, d_obj.day
        hour = int(gd["H"]) if gd.get("H") else 0
        if gd.get("I"):
            hour = int(gd["I"]) % 12
            if gd.get("p") and gd["p"].upper() == "PM":
                hour += 12
        minute = int(gd["M"]) if gd.get("M") else 0
        second = int(gd["S"]) if gd.get("S") else 0
        microsecond = _parse_frac_us(gd["f"]) if gd.get("f") else 0
        tz = _parse_iso_tz(gd["z"]) if gd.get("z") else None
        if tz is None and gd.get("Z") in ("UTC", "GMT", "Z"):
            tz = timezone.utc
        return cls(year, month, day, hour, minute, second, microsecond, tz)

    def date(self):
        return date(self._year, self._month, self._day)

    def time(self):
        return time(self._hour, self._minute, self._second, self._microsecond, fold=self._fold)

    def timetz(self):
        return time(self._hour, self._minute, self._second, self._microsecond, self._tzinfo, fold=self._fold)

    def utcoffset(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.utcoffset(self)

    def dst(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.dst(self)

    def tzname(self):
        if self._tzinfo is None:
            return None
        return self._tzinfo.tzname(self)

    def replace(self, year=None, month=None, day=None, hour=None, minute=None, second=None, microsecond=None, tzinfo=True, *, fold=None):
        return type(self)(
            self._year if year is None else year,
            self._month if month is None else month,
            self._day if day is None else day,
            self._hour if hour is None else hour,
            self._minute if minute is None else minute,
            self._second if second is None else second,
            self._microsecond if microsecond is None else microsecond,
            self._tzinfo if tzinfo is True else tzinfo,
            fold=self._fold if fold is None else fold,
        )

    def astimezone(self, tz=None):
        if tz is None:
            tz = timezone.utc
        _check_tz(tz)
        my_off = self.utcoffset()
        if my_off is None:
            my_off = timedelta(0)
        utc_us = self._local_us() - my_off._us
        utc_dt = datetime._from_local_us(utc_us, tz)
        return tz.fromutc(utc_dt)

    def timestamp(self):
        off = self.utcoffset()
        utc_us = self._local_us() - (off._us if off is not None else 0)
        return (utc_us - (_EPOCH_ORD - 1) * _US_PER_DAY) / _US_PER_SEC

    def isoformat(self, sep="T", timespec="auto"):
        d = "%04d-%02d-%02d" % (self._year, self._month, self._day)
        t = _fmt_time_part(self._hour, self._minute, self._second, self._microsecond, timespec)
        return d + sep + t + _fmt_offset(self.utcoffset(), colon=True)

    def __str__(self):
        return self.isoformat(sep=" ")

    def ctime(self):
        return "%s %s %2d %02d:%02d:%02d %04d" % (
            _WD_SHORT[self.weekday()], _MON_SHORT[self._month], self._day,
            self._hour, self._minute, self._second, self._year,
        )

    def timetuple(self):
        yday = self.toordinal() - _ymd_to_ord(self._year, 1, 1) + 1
        d = self.dst()
        isdst = -1 if d is None else (1 if d._us != 0 else 0)
        return (self._year, self._month, self._day, self._hour, self._minute, self._second, self.weekday(), yday, isdst)

    def utctimetuple(self):
        off = self.utcoffset()
        dt = self if off is None else datetime._from_local_us(self._local_us() - off._us, None)
        yday = dt.toordinal() - _ymd_to_ord(dt._year, 1, 1) + 1
        return (dt._year, dt._month, dt._day, dt._hour, dt._minute, dt._second, dt.weekday(), yday, 0)

    def __repr__(self):
        parts = [str(self._year), str(self._month), str(self._day), str(self._hour), str(self._minute)]
        if self._second or self._microsecond:
            parts.append(str(self._second))
        if self._microsecond:
            parts.append(str(self._microsecond))
        if self._tzinfo is not None:
            parts.append("tzinfo=%r" % (self._tzinfo,))
        if self._fold:
            parts.append("fold=%d" % self._fold)
        return "datetime.datetime(%s)" % ", ".join(parts)

    def _cmp_us(self, other, eq_only=False):
        o1 = self.utcoffset()
        o2 = other.utcoffset()
        if (o1 is None) != (o2 is None):
            if eq_only:
                return None, None
            raise TypeError("can't compare offset-naive and offset-aware datetimes")
        if o1 is None:
            return self._local_us(), other._local_us()
        return self._local_us() - o1._us, other._local_us() - o2._us

    def __eq__(self, other):
        if not isinstance(other, datetime):
            if isinstance(other, date):
                return False
            return NotImplemented
        a, b = self._cmp_us(other, eq_only=True)
        return False if a is None else a == b

    def __lt__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_us(other)
        return a < b

    def __le__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_us(other)
        return a <= b

    def __gt__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_us(other)
        return a > b

    def __ge__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_us(other)
        return a >= b

    def __hash__(self):
        off = self.utcoffset()
        if off is None:
            return hash(self._local_us())
        return hash(self._local_us() - off._us)

    def __add__(self, other):
        if isinstance(other, timedelta):
            return datetime._from_local_us(self._local_us() + other._us, self._tzinfo)
        return NotImplemented

    __radd__ = __add__

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return datetime._from_local_us(self._local_us() - other._us, self._tzinfo)
        if isinstance(other, datetime):
            o1 = self.utcoffset()
            o2 = other.utcoffset()
            if (o1 is None) != (o2 is None):
                raise TypeError("can't subtract offset-naive and offset-aware datetimes")
            if o1 is None:
                return timedelta._from_us(self._local_us() - other._local_us())
            return timedelta._from_us((self._local_us() - o1._us) - (other._local_us() - o2._us))
        return NotImplemented


datetime.min = datetime(MINYEAR, 1, 1)
datetime.max = datetime(MAXYEAR, 12, 31, 23, 59, 59, 999999)
datetime.resolution = timedelta(microseconds=1)
