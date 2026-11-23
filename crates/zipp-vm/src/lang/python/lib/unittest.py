"""Unit testing framework supporting TestCase, TestSuite, TestLoader, and runner."""

import asyncio
import collections
import contextlib
import functools
import inspect
import logging
import re
import sys
import time
import traceback
import warnings


class SkipTest(Exception):
    """Raised to mark a test as skipped."""


class _ExpectedFailure(Exception):
    def __init__(self, exc_info):
        super().__init__()
        self.exc_info = exc_info


class _UnexpectedSuccess(Exception):
    pass


def skip(reason):
    """Unconditionally skip a test function or class."""
    def decorator(target):
        if isinstance(target, type):
            target.__unittest_skip__ = True
            target.__unittest_skip_why__ = reason
            return target

        @functools.wraps(target)
        def wrapper(*args, **kwargs):
            raise SkipTest(reason)

        wrapper.__unittest_skip__ = True
        wrapper.__unittest_skip_why__ = reason
        return wrapper

    return decorator


def skipIf(condition, reason):
    if condition:
        return skip(reason)
    return lambda obj: obj


def skipUnless(condition, reason):
    if not condition:
        return skip(reason)
    return lambda obj: obj


def expectedFailure(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        try:
            func(*args, **kwargs)
        except Exception:
            raise _ExpectedFailure(sys.exc_info())
        raise _UnexpectedSuccess()

    wrapper.__unittest_expecting_failure__ = True
    return wrapper


def _safe_repr(obj, short=False):
    try:
        text = repr(obj)
    except Exception:
        text = object.__repr__(obj)
    if short and len(text) > 80:
        return text[:77] + "..."
    return text


def _strclass(cls):
    return "{}.{}".format(cls.__module__, cls.__qualname__)


class _AssertRaisesContext:
    def __init__(self, expected, test_case, expected_regex=None):
        self.expected = expected
        self.test_case = test_case
        self.expected_regex = (
            re.compile(expected_regex)
            if isinstance(expected_regex, (str, bytes))
            else expected_regex
        )
        self.exception = None
        self.obj_name = None

    def _raise_failure(self, standard_msg):
        msg = self.test_case._formatMessage(self.msg, standard_msg)
        raise self.test_case.failureException(msg)

    def handle(self, name, args, kwargs):
        if not args:
            self.msg = kwargs.pop("msg", None)
            if kwargs:
                raise TypeError(
                    "{}() got unexpected keyword argument(s): {}".format(
                        name, ", ".join(kwargs)
                    )
                )
            return self
        callable_obj = args[0]
        call_args = args[1:]
        self.msg = None
        self.obj_name = getattr(callable_obj, "__name__", str(callable_obj))
        with self:
            callable_obj(*call_args, **kwargs)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, tb):
        if exc_type is None:
            try:
                exc_name = self.expected.__name__
            except AttributeError:
                exc_name = str(self.expected)
            if self.obj_name:
                self._raise_failure("{} not raised by {}".format(exc_name, self.obj_name))
            else:
                self._raise_failure("{} not raised".format(exc_name))
        if not issubclass(exc_type, self.expected):
            return False
        self.exception = exc_value
        if self.expected_regex is not None:
            if not self.expected_regex.search(str(exc_value)):
                self._raise_failure(
                    "{!r} does not match {!r}".format(
                        self.expected_regex.pattern, str(exc_value)
                    )
                )
        return True


class _AssertWarnsContext:
    def __init__(self, expected, test_case, expected_regex=None):
        self.expected = expected
        self.test_case = test_case
        self.expected_regex = (
            re.compile(expected_regex)
            if isinstance(expected_regex, (str, bytes))
            else expected_regex
        )
        self.warning = None
        self.warnings = []
        self.filename = None
        self.lineno = None
        self._cm = None
        self.msg = None

    def handle(self, name, args, kwargs):
        if not args:
            self.msg = kwargs.pop("msg", None)
            return self
        callable_obj = args[0]
        call_args = args[1:]
        with self:
            return callable_obj(*call_args, **kwargs)

    def __enter__(self):
        self._cm = warnings.catch_warnings(record=True)
        self.warnings = self._cm.__enter__()
        warnings.simplefilter("always", self.expected)
        return self

    def __exit__(self, exc_type, exc_value, tb):
        self._cm.__exit__(exc_type, exc_value, tb)
        if exc_type is not None:
            return False
        first_matching = None
        for rec in self.warnings:
            w = rec.message
            if not isinstance(w, self.expected):
                continue
            if first_matching is None:
                first_matching = rec
            if self.expected_regex is None or self.expected_regex.search(str(w)):
                self.warning = w
                self.filename = rec.filename
                self.lineno = rec.lineno
                return False
        try:
            exc_name = self.expected.__name__
        except AttributeError:
            exc_name = str(self.expected)
        if first_matching is not None and self.expected_regex is not None:
            msg = "{!r} does not match {!r}".format(
                self.expected_regex.pattern, str(first_matching.message)
            )
        else:
            msg = "{} not triggered".format(exc_name)
        raise self.test_case.failureException(self.test_case._formatMessage(self.msg, msg))


class _CapturingHandler(logging.Handler):
    def __init__(self):
        super().__init__()
        self.watcher = _LoggingWatcher([], [])

    def emit(self, record):
        self.watcher.records.append(record)
        self.watcher.output.append(self.format(record))


class _LoggingWatcher:
    def __init__(self, records, output):
        self.records = records
        self.output = output


class _AssertLogsContext:
    def __init__(self, test_case, logger_name, level, no_logs=False):
        self.test_case = test_case
        self.logger_name = logger_name
        self.level = level if level is not None else logging.INFO
        self.no_logs = no_logs
        self.logger = None
        self._old_handlers = None
        self._old_level = None
        self._old_propagate = None
        self._handler = None

    def __enter__(self):
        if isinstance(self.logger_name, logging.Logger):
            logger = self.logger_name
        else:
            logger = logging.getLogger(self.logger_name)
        self.logger = logger
        self._old_handlers = list(logger.handlers)
        self._old_level = logger.level
        self._old_propagate = logger.propagate
        handler = _CapturingHandler()
        handler.setFormatter(logging.Formatter("%(levelname)s:%(name)s:%(message)s"))
        self._handler = handler
        logger.handlers = [handler]
        if isinstance(self.level, str):
            lvl = getattr(logging, self.level.upper(), logging.INFO)
        else:
            lvl = self.level
        logger.setLevel(lvl)
        logger.propagate = False
        return handler.watcher

    def __exit__(self, exc_type, exc_value, tb):
        self.logger.handlers = self._old_handlers
        self.logger.setLevel(self._old_level)
        self.logger.propagate = self._old_propagate
        if exc_type is not None:
            return False
        records = self._handler.watcher.records
        if self.no_logs:
            if records:
                raise self.test_case.failureException(
                    "Unexpected logs found: {!r}".format(self._handler.watcher.output)
                )
        else:
            if not records:
                raise self.test_case.failureException(
                    "no logs of level {} or higher triggered on {}".format(
                        self.level, self.logger.name
                    )
                )
        return False


class _SubTest:
    def __init__(self, test_case, message, params):
        self.test_case = test_case
        self._message = message
        self.params = params

    def id(self):
        parts = [self.test_case.id()]
        if self._message is not _SUBTEST_SENTINEL:
            parts.append("[{}]".format(self._message))
        if self.params:
            kv = ", ".join("{}={!r}".format(k, v) for k, v in self.params.items())
            parts.append("({})".format(kv))
        return " ".join(parts)

    def __str__(self):
        return self.id()


_SUBTEST_SENTINEL = object()


class TestCase:
    """Base class for test cases."""

    failureException = AssertionError
    longMessage = True
    maxDiff = 640
    _class_cleanups = []

    def __init__(self, methodName="runTest"):
        self._testMethodName = methodName
        self._cleanups = []
        self._subtest = None
        self._outcome_result = None
        self._type_equality_funcs = {}
        self.addTypeEqualityFunc(dict, "assertDictEqual")
        self.addTypeEqualityFunc(list, "assertListEqual")
        self.addTypeEqualityFunc(tuple, "assertTupleEqual")
        self.addTypeEqualityFunc(set, "assertSetEqual")
        self.addTypeEqualityFunc(frozenset, "assertSetEqual")
        self.addTypeEqualityFunc(str, "assertMultiLineEqual")
        if methodName != "runTest":
            try:
                test_method = getattr(self, methodName)
            except AttributeError:
                raise ValueError(
                    "no such test method in {}: {}".format(type(self), methodName)
                )
            self._testMethodDoc = getattr(test_method, "__doc__", None)
        else:
            self._testMethodDoc = getattr(self.runTest, "__doc__", None)

    def addTypeEqualityFunc(self, typeobj, function):
        self._type_equality_funcs[typeobj] = function

    def addCleanup(self, function, /, *args, **kwargs):
        self._cleanups.append((function, args, kwargs))

    @classmethod
    def addClassCleanup(cls, function, /, *args, **kwargs):
        if "_class_cleanups" not in cls.__dict__:
            cls._class_cleanups = []
        cls._class_cleanups.append((function, args, kwargs))

    def setUp(self):
        pass

    def tearDown(self):
        pass

    @classmethod
    def setUpClass(cls):
        pass

    @classmethod
    def tearDownClass(cls):
        pass

    def runTest(self):
        pass

    def countTestCases(self):
        return 1

    def defaultTestResult(self):
        return TestResult()

    def shortDescription(self):
        doc = self._testMethodDoc
        if doc:
            return doc.strip().splitlines()[0].strip()
        return None

    def id(self):
        return "{}.{}".format(_strclass(type(self)), self._testMethodName)

    def __eq__(self, other):
        if type(self) is not type(other):
            return NotImplemented
        return self._testMethodName == other._testMethodName

    def __hash__(self):
        return hash((type(self), self._testMethodName))

    def __str__(self):
        return "{} ({})".format(self._testMethodName, _strclass(type(self)))

    def __repr__(self):
        return "<{} testMethod={}>".format(_strclass(type(self)), self._testMethodName)

    def _formatMessage(self, msg, standardMsg):
        if not self.longMessage:
            return msg or standardMsg
        if msg is None:
            return standardMsg
        return "{} : {}".format(standardMsg, msg)

    def doCleanups(self):
        ok = True
        while self._cleanups:
            func, args, kwargs = self._cleanups.pop()
            try:
                func(*args, **kwargs)
            except Exception:
                ok = False
                if self._outcome_result is not None:
                    self._outcome_result.addError(self, sys.exc_info())
                else:
                    raise
        return ok

    @classmethod
    def doClassCleanups(cls):
        cleanups = cls.__dict__.get("_class_cleanups", [])
        exceptions = []
        while cleanups:
            func, args, kwargs = cleanups.pop()
            try:
                func(*args, **kwargs)
            except Exception as exc:
                exceptions.append(exc)
        if exceptions:
            raise exceptions[0]

    @contextlib.contextmanager
    def subTest(self, msg=_SUBTEST_SENTINEL, **params):
        if self._outcome_result is None:
            yield
            return
        parent = self._subtest
        sub = _SubTest(self, msg, params)
        self._subtest = sub
        try:
            yield
        except SkipTest as exc:
            self._outcome_result.addSubTest(self, sub, sys.exc_info())
            _ = exc
        except Exception:
            self._outcome_result.addSubTest(self, sub, sys.exc_info())
        else:
            self._outcome_result.addSubTest(self, sub, None)
        finally:
            self._subtest = parent

    def _callTestMethod(self, method):
        return method()

    def run(self, result=None):
        if result is None:
            result = self.defaultTestResult()
        self._outcome_result = result
        result.startTest(self)
        test_method = getattr(self, self._testMethodName)
        if getattr(type(self), "__unittest_skip__", False) or getattr(
            test_method, "__unittest_skip__", False
        ):
            reason = getattr(type(self), "__unittest_skip_why__", "") or getattr(
                test_method, "__unittest_skip_why__", ""
            )
            result.addSkip(self, reason)
            result.stopTest(self)
            self._outcome_result = None
            return result

        success = False
        try:
            try:
                self.setUp()
            except SkipTest as exc:
                result.addSkip(self, str(exc))
                return result
            except KeyboardInterrupt:
                raise
            except Exception:
                result.addError(self, sys.exc_info())
                return result

            try:
                self._callTestMethod(test_method)
                success = True
            except SkipTest as exc:
                result.addSkip(self, str(exc))
            except _ExpectedFailure as exc:
                result.addExpectedFailure(self, exc.exc_info)
            except _UnexpectedSuccess:
                result.addUnexpectedSuccess(self)
            except self.failureException:
                result.addFailure(self, sys.exc_info())
            except KeyboardInterrupt:
                raise
            except Exception:
                result.addError(self, sys.exc_info())
            finally:
                try:
                    self.tearDown()
                except KeyboardInterrupt:
                    raise
                except Exception:
                    result.addError(self, sys.exc_info())
                    success = False
        finally:
            clean_ok = self.doCleanups()
            if success and clean_ok and not getattr(result, "_subtest_failed", False):
                result.addSuccess(self)
            if hasattr(result, "_subtest_failed"):
                result._subtest_failed = False
            result.stopTest(self)
            self._outcome_result = None
        return result

    def __call__(self, *args, **kwargs):
        return self.run(*args, **kwargs)

    def debug(self):
        self.setUp()
        getattr(self, self._testMethodName)()
        self.tearDown()
        self.doCleanups()

    def skipTest(self, reason):
        raise SkipTest(reason)

    def fail(self, msg=None):
        raise self.failureException(msg)

    def assertFalse(self, expr, msg=None):
        if expr:
            raise self.failureException(
                self._formatMessage(msg, "{} is not false".format(_safe_repr(expr)))
            )

    def assertTrue(self, expr, msg=None):
        if not expr:
            raise self.failureException(
                self._formatMessage(msg, "{} is not true".format(_safe_repr(expr)))
            )

    def assertRaises(self, expected_exception, *args, **kwargs):
        ctx = _AssertRaisesContext(expected_exception, self)
        return ctx.handle("assertRaises", args, kwargs)

    def assertRaisesRegex(self, expected_exception, expected_regex, *args, **kwargs):
        ctx = _AssertRaisesContext(expected_exception, self, expected_regex)
        return ctx.handle("assertRaisesRegex", args, kwargs)

    def assertWarns(self, expected_warning, *args, **kwargs):
        ctx = _AssertWarnsContext(expected_warning, self)
        return ctx.handle("assertWarns", args, kwargs)

    def assertWarnsRegex(self, expected_warning, expected_regex, *args, **kwargs):
        ctx = _AssertWarnsContext(expected_warning, self, expected_regex)
        return ctx.handle("assertWarnsRegex", args, kwargs)

    def assertLogs(self, logger=None, level=None):
        return _AssertLogsContext(self, logger, level, no_logs=False)

    def assertNoLogs(self, logger=None, level=None):
        return _AssertLogsContext(self, logger, level, no_logs=True)

    def _baseAssertEqual(self, first, second, msg=None):
        if not first == second:
            standard = "{} != {}".format(_safe_repr(first), _safe_repr(second))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertEqual(self, first, second, msg=None):
        if type(first) is type(second):
            asserter = self._type_equality_funcs.get(type(first))
            if asserter is not None:
                if isinstance(asserter, str):
                    asserter = getattr(self, asserter)
                asserter(first, second, msg=msg)
                return
        self._baseAssertEqual(first, second, msg=msg)

    def assertNotEqual(self, first, second, msg=None):
        if not first != second:
            standard = "{} == {}".format(_safe_repr(first), _safe_repr(second))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertAlmostEqual(self, first, second, places=None, msg=None, delta=None):
        if first == second:
            return
        if delta is not None and places is not None:
            raise TypeError("specify delta or places not both")
        diff = abs(first - second)
        if delta is not None:
            if diff <= delta:
                return
            standard = "{} != {} within {} delta ({} difference)".format(
                _safe_repr(first), _safe_repr(second), _safe_repr(delta), _safe_repr(diff)
            )
        else:
            if places is None:
                places = 7
            if round(diff, places) == 0:
                return
            standard = "{} != {} within {} places ({} difference)".format(
                _safe_repr(first), _safe_repr(second), places, _safe_repr(diff)
            )
        raise self.failureException(self._formatMessage(msg, standard))

    def assertNotAlmostEqual(self, first, second, places=None, msg=None, delta=None):
        if delta is not None and places is not None:
            raise TypeError("specify delta or places not both")
        diff = abs(first - second)
        if delta is not None:
            if not (first == second) and diff > delta:
                return
            standard = "{} == {} within {} delta ({} difference)".format(
                _safe_repr(first), _safe_repr(second), _safe_repr(delta), _safe_repr(diff)
            )
        else:
            if places is None:
                places = 7
            if not (first == second) and round(diff, places) != 0:
                return
            standard = "{} == {} within {} places".format(
                _safe_repr(first), _safe_repr(second), places
            )
        raise self.failureException(self._formatMessage(msg, standard))

    def assertSequenceEqual(self, seq1, seq2, msg=None, seq_type=None):
        if seq_type is not None:
            if not isinstance(seq1, seq_type):
                raise self.failureException(
                    self._formatMessage(
                        msg,
                        "First sequence is not a {}: {}".format(
                            seq_type.__name__, _safe_repr(seq1)
                        ),
                    )
                )
            if not isinstance(seq2, seq_type):
                raise self.failureException(
                    self._formatMessage(
                        msg,
                        "Second sequence is not a {}: {}".format(
                            seq_type.__name__, _safe_repr(seq2)
                        ),
                    )
                )
        if seq1 != seq2:
            standard = "{} != {}".format(_safe_repr(seq1), _safe_repr(seq2))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertListEqual(self, list1, list2, msg=None):
        self.assertSequenceEqual(list1, list2, msg=msg, seq_type=list)

    def assertTupleEqual(self, tuple1, tuple2, msg=None):
        self.assertSequenceEqual(tuple1, tuple2, msg=msg, seq_type=tuple)

    def assertSetEqual(self, set1, set2, msg=None):
        if not isinstance(set1, (set, frozenset)):
            raise self.failureException(
                self._formatMessage(msg, "First argument is not a set")
            )
        if not isinstance(set2, (set, frozenset)):
            raise self.failureException(
                self._formatMessage(msg, "Second argument is not a set")
            )
        if set1 != set2:
            standard = "{} != {}".format(_safe_repr(set1), _safe_repr(set2))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertDictEqual(self, d1, d2, msg=None):
        self.assertIsInstance(d1, dict, "First argument is not a dictionary")
        self.assertIsInstance(d2, dict, "Second argument is not a dictionary")
        if d1 != d2:
            standard = "{} != {}".format(_safe_repr(d1), _safe_repr(d2))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertMultiLineEqual(self, first, second, msg=None):
        self.assertIsInstance(first, str, "First argument is not a string")
        self.assertIsInstance(second, str, "Second argument is not a string")
        if first != second:
            standard = "{} != {}".format(_safe_repr(first), _safe_repr(second))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertCountEqual(self, first, second, msg=None):
        seq1 = list(first)
        seq2 = list(second)
        try:
            equal = collections.Counter(seq1) == collections.Counter(seq2)
        except TypeError:
            rem = list(seq2)
            equal = True
            for item in seq1:
                found = False
                for idx, other in enumerate(rem):
                    if item == other:
                        del rem[idx]
                        found = True
                        break
                if not found:
                    equal = False
                    break
            if rem:
                equal = False
        if not equal:
            standard = "Element counts were not equal: {} != {}".format(
                _safe_repr(seq1), _safe_repr(seq2)
            )
            raise self.failureException(self._formatMessage(msg, standard))

    def assertIn(self, member, container, msg=None):
        if member not in container:
            standard = "{} not found in {}".format(_safe_repr(member), _safe_repr(container))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertNotIn(self, member, container, msg=None):
        if member in container:
            standard = "{} unexpectedly found in {}".format(
                _safe_repr(member), _safe_repr(container)
            )
            raise self.failureException(self._formatMessage(msg, standard))

    def assertIs(self, expr1, expr2, msg=None):
        if expr1 is not expr2:
            standard = "{} is not {}".format(_safe_repr(expr1), _safe_repr(expr2))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertIsNot(self, expr1, expr2, msg=None):
        if expr1 is expr2:
            standard = "unexpectedly identical: {}".format(_safe_repr(expr1))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertIsNone(self, obj, msg=None):
        if obj is not None:
            standard = "{} is not None".format(_safe_repr(obj))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertIsNotNone(self, obj, msg=None):
        if obj is None:
            raise self.failureException(self._formatMessage(msg, "unexpectedly None"))

    def assertIsInstance(self, obj, cls, msg=None):
        if not isinstance(obj, cls):
            standard = "{} is not an instance of {}".format(
                _safe_repr(obj), _safe_repr(cls)
            )
            raise self.failureException(self._formatMessage(msg, standard))

    def assertNotIsInstance(self, obj, cls, msg=None):
        if isinstance(obj, cls):
            standard = "{} is an instance of {}".format(_safe_repr(obj), _safe_repr(cls))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertLess(self, a, b, msg=None):
        if not a < b:
            standard = "{} not less than {}".format(_safe_repr(a), _safe_repr(b))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertLessEqual(self, a, b, msg=None):
        if not a <= b:
            standard = "{} not less than or equal to {}".format(_safe_repr(a), _safe_repr(b))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertGreater(self, a, b, msg=None):
        if not a > b:
            standard = "{} not greater than {}".format(_safe_repr(a), _safe_repr(b))
            raise self.failureException(self._formatMessage(msg, standard))

    def assertGreaterEqual(self, a, b, msg=None):
        if not a >= b:
            standard = "{} not greater than or equal to {}".format(
                _safe_repr(a), _safe_repr(b)
            )
            raise self.failureException(self._formatMessage(msg, standard))

    def assertRegex(self, text, expected_regex, msg=None):
        if isinstance(expected_regex, (str, bytes)):
            expected_regex = re.compile(expected_regex)
        if not expected_regex.search(text):
            standard = "Regex didn't match: {!r} not found in {!r}".format(
                expected_regex.pattern, text
            )
            raise self.failureException(self._formatMessage(msg, standard))

    def assertNotRegex(self, text, unexpected_regex, msg=None):
        if isinstance(unexpected_regex, (str, bytes)):
            unexpected_regex = re.compile(unexpected_regex)
        match = unexpected_regex.search(text)
        if match:
            standard = "Regex matched: {!r} matches {!r} in {!r}".format(
                text[match.start() : match.end()], unexpected_regex.pattern, text
            )
            raise self.failureException(self._formatMessage(msg, standard))

    # Legacy aliases kept for backwards compatibility.
    failUnless = assertTrue
    failIf = assertFalse
    assertEquals = assertEqual
    assertNotEquals = assertNotEqual
    assertAlmostEquals = assertAlmostEqual
    assertNotAlmostEquals = assertNotAlmostEqual
    assertRegexpMatches = assertRegex
    assertNotRegexpMatches = assertNotRegex
    assertRaisesRegexp = assertRaisesRegex


class IsolatedAsyncioTestCase(TestCase):
    """TestCase subclass that runs async test methods on a fresh event loop."""

    async def asyncSetUp(self):
        pass

    async def asyncTearDown(self):
        pass

    def _callTestMethod(self, method):
        async def _runner():
            await self.asyncSetUp()
            try:
                res = method()
                if inspect.isawaitable(res):
                    await res
            finally:
                await self.asyncTearDown()

        return asyncio.run(_runner())


class FunctionTestCase(TestCase):
    """Wrap a standalone callable as a TestCase."""

    def __init__(self, testFunc, setUp=None, tearDown=None, description=None):
        super().__init__()
        self._testFunc = testFunc
        self._setUpFunc = setUp
        self._tearDownFunc = tearDown
        self._description = description

    def setUp(self):
        if self._setUpFunc is not None:
            self._setUpFunc()

    def tearDown(self):
        if self._tearDownFunc is not None:
            self._tearDownFunc()

    def runTest(self):
        self._testFunc()

    def shortDescription(self):
        if self._description is not None:
            return self._description
        doc = getattr(self._testFunc, "__doc__", None)
        return doc.strip().splitlines()[0].strip() if doc else None


class TestResult:
    """Collects outcomes from running tests."""

    def __init__(self, stream=None, descriptions=None, verbosity=None):
        self.failfast = False
        self.failures = []
        self.errors = []
        self.testsRun = 0
        self.skipped = []
        self.expectedFailures = []
        self.unexpectedSuccesses = []
        self.shouldStop = False
        self.buffer = False
        self.tb_locals = False
        self._subtest_failed = False

    def startTest(self, test):
        self.testsRun += 1

    def stopTest(self, test):
        pass

    def startTestRun(self):
        pass

    def stopTestRun(self):
        pass

    def _exc_info_to_string(self, err, test):
        return "".join(traceback.format_exception(*err))

    def addError(self, test, err):
        self.errors.append((test, self._exc_info_to_string(err, test)))
        if self.failfast:
            self.stop()

    def addFailure(self, test, err):
        self.failures.append((test, self._exc_info_to_string(err, test)))
        if self.failfast:
            self.stop()

    def addSubTest(self, test, subtest, err):
        if err is not None:
            self._subtest_failed = True
            if issubclass(err[0], SkipTest):
                self.skipped.append((subtest, str(err[1])))
            elif issubclass(err[0], test.failureException):
                self.failures.append((subtest, self._exc_info_to_string(err, subtest)))
                if self.failfast:
                    self.stop()
            else:
                self.errors.append((subtest, self._exc_info_to_string(err, subtest)))
                if self.failfast:
                    self.stop()

    def addSuccess(self, test):
        pass

    def addSkip(self, test, reason):
        self.skipped.append((test, reason))

    def addExpectedFailure(self, test, err):
        self.expectedFailures.append((test, self._exc_info_to_string(err, test)))

    def addUnexpectedSuccess(self, test):
        self.unexpectedSuccesses.append(test)

    def wasSuccessful(self):
        return (
            len(self.failures) == 0
            and len(self.errors) == 0
            and len(self.unexpectedSuccesses) == 0
        )

    def stop(self):
        self.shouldStop = True

    def __repr__(self):
        return "<{} run={} errors={} failures={}>".format(
            _strclass(type(self)), self.testsRun, len(self.errors), len(self.failures)
        )


class TextTestResult(TestResult):
    separator1 = "=" * 70
    separator2 = "-" * 70

    def __init__(self, stream, descriptions, verbosity):
        super().__init__(stream, descriptions, verbosity)
        self.stream = stream
        self.showAll = verbosity > 1
        self.dots = verbosity == 1
        self.descriptions = descriptions

    def getDescription(self, test):
        doc_line = test.shortDescription() if hasattr(test, "shortDescription") else None
        if self.descriptions and doc_line:
            return "{}\n{}".format(test, doc_line)
        return str(test)

    def startTest(self, test):
        super().startTest(test)
        if self.showAll:
            self.stream.write(self.getDescription(test))
            self.stream.write(" ... ")

    def addSuccess(self, test):
        super().addSuccess(test)
        if self.showAll:
            self.stream.write("ok\n")
        elif self.dots:
            self.stream.write(".")

    def addError(self, test, err):
        super().addError(test, err)
        if self.showAll:
            self.stream.write("ERROR\n")
        elif self.dots:
            self.stream.write("E")

    def addFailure(self, test, err):
        super().addFailure(test, err)
        if self.showAll:
            self.stream.write("FAIL\n")
        elif self.dots:
            self.stream.write("F")

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        if self.showAll:
            self.stream.write("skipped {!r}\n".format(reason))
        elif self.dots:
            self.stream.write("s")

    def addExpectedFailure(self, test, err):
        super().addExpectedFailure(test, err)
        if self.showAll:
            self.stream.write("expected failure\n")
        elif self.dots:
            self.stream.write("x")

    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test)
        if self.showAll:
            self.stream.write("unexpected success\n")
        elif self.dots:
            self.stream.write("u")

    def printErrors(self):
        if self.dots or self.showAll:
            self.stream.write("\n")
        self.printErrorList("ERROR", self.errors)
        self.printErrorList("FAIL", self.failures)

    def printErrorList(self, flavour, errors):
        for test, err in errors:
            self.stream.write(self.separator1 + "\n")
            self.stream.write("{}: {}\n".format(flavour, self.getDescription(test)))
            self.stream.write(self.separator2 + "\n")
            self.stream.write(err + "\n")


class TestSuite:
    """Ordered collection of TestCase and nested TestSuite objects."""

    def __init__(self, tests=()):
        self._tests = []
        self.addTests(tests)

    def addTest(self, test):
        if not callable(test):
            raise TypeError("{} is not callable".format(repr(test)))
        self._tests.append(test)

    def addTests(self, tests):
        for test in tests:
            self.addTest(test)

    def __iter__(self):
        return iter(self._tests)

    def countTestCases(self):
        total = 0
        for test in self._tests:
            total += test.countTestCases()
        return total

    def _run_with_class_fixtures(self, result, prev_class_holder):
        for test in self._tests:
            if result.shouldStop:
                break
            if isinstance(test, TestSuite):
                test._run_with_class_fixtures(result, prev_class_holder)
                continue
            cls = type(test)
            prev_cls = prev_class_holder[0]
            if cls is not prev_cls:
                if prev_cls is not None and not getattr(prev_cls, "__unittest_skip__", False):
                    try:
                        prev_cls.tearDownClass()
                    except Exception:
                        result.addError(prev_cls, sys.exc_info())
                    try:
                        prev_cls.doClassCleanups()
                    except Exception:
                        result.addError(prev_cls, sys.exc_info())
                prev_class_holder[0] = cls
                if not getattr(cls, "__unittest_skip__", False):
                    try:
                        cls.setUpClass()
                    except SkipTest as exc:
                        cls._class_setup_skipped = str(exc)
                    except Exception:
                        cls._class_setup_failed = sys.exc_info()
            if getattr(cls, "_class_setup_skipped", None) is not None:
                result.startTest(test)
                result.addSkip(test, cls._class_setup_skipped)
                result.stopTest(test)
                continue
            if getattr(cls, "_class_setup_failed", None) is not None:
                result.addError(test, cls._class_setup_failed)
                continue
            test(result)

    def run(self, result):
        prev_class_holder = [None]
        try:
            self._run_with_class_fixtures(result, prev_class_holder)
        finally:
            prev_cls = prev_class_holder[0]
            if prev_cls is not None and not getattr(prev_cls, "__unittest_skip__", False):
                if getattr(prev_cls, "_class_setup_failed", None) is None and getattr(
                    prev_cls, "_class_setup_skipped", None
                ) is None:
                    try:
                        prev_cls.tearDownClass()
                    except Exception:
                        result.addError(prev_cls, sys.exc_info())
                    try:
                        prev_cls.doClassCleanups()
                    except Exception:
                        result.addError(prev_cls, sys.exc_info())
                for attr in ("_class_setup_failed", "_class_setup_skipped"):
                    if hasattr(prev_cls, attr):
                        try:
                            delattr(prev_cls, attr)
                        except Exception:
                            pass
        return result

    def __call__(self, *args, **kwargs):
        return self.run(*args, **kwargs)


class TestLoader:
    testMethodPrefix = "test"
    suiteClass = TestSuite

    def getTestCaseNames(self, testCaseClass):
        names = []
        for attr in dir(testCaseClass):
            if attr.startswith(self.testMethodPrefix) and callable(
                getattr(testCaseClass, attr)
            ):
                names.append(attr)
        names.sort()
        return names

    def loadTestsFromTestCase(self, testCaseClass):
        if issubclass(testCaseClass, TestSuite):
            raise TypeError("Test cases should not be derived from TestSuite.")
        names = self.getTestCaseNames(testCaseClass)
        if not names and hasattr(testCaseClass, "runTest"):
            names = ["runTest"]
        return self.suiteClass([testCaseClass(name) for name in names])

    def loadTestsFromModule(self, module, *args, pattern=None, **kwargs):
        tests = []
        for name in dir(module):
            obj = getattr(module, name)
            if isinstance(obj, type) and issubclass(obj, TestCase) and obj is not TestCase:
                if obj is IsolatedAsyncioTestCase or obj is FunctionTestCase:
                    continue
                tests.append(self.loadTestsFromTestCase(obj))
        return self.suiteClass(tests)

    def loadTestsFromName(self, name, module=None):
        parts = name.split(".")
        obj = module
        if obj is None:
            import importlib

            obj = importlib.import_module(parts[0])
            parts = parts[1:]
        parent = None
        for part in parts:
            parent = obj
            obj = getattr(obj, part)
        if isinstance(obj, type) and issubclass(obj, TestCase):
            return self.loadTestsFromTestCase(obj)
        if callable(obj) and parent is not None and isinstance(parent, type) and issubclass(parent, TestCase):
            return self.suiteClass([parent(parts[-1])])
        if isinstance(obj, TestSuite):
            return obj
        if hasattr(obj, "__dict__"):
            return self.loadTestsFromModule(obj)
        raise TypeError("don't know how to make test from: {}".format(obj))

    def loadTestsFromNames(self, names, module=None):
        suites = [self.loadTestsFromName(name, module) for name in names]
        return self.suiteClass(suites)


defaultTestLoader = TestLoader()


class TextTestRunner:
    resultclass = TextTestResult

    def __init__(
        self,
        stream=None,
        descriptions=True,
        verbosity=1,
        failfast=False,
        buffer=False,
        resultclass=None,
        warnings=None,
        *,
        tb_locals=False,
    ):
        self.stream = stream if stream is not None else sys.stderr
        self.descriptions = descriptions
        self.verbosity = verbosity
        self.failfast = failfast
        self.buffer = buffer
        self.tb_locals = tb_locals
        self.warnings = warnings
        if resultclass is not None:
            self.resultclass = resultclass

    def _makeResult(self):
        return self.resultclass(self.stream, self.descriptions, self.verbosity)

    def run(self, test):
        result = self._makeResult()
        result.failfast = self.failfast
        result.buffer = self.buffer
        result.tb_locals = self.tb_locals
        start = time.perf_counter()
        result.startTestRun()
        try:
            test(result)
        finally:
            result.stopTestRun()
        elapsed = time.perf_counter() - start
        if hasattr(result, "printErrors"):
            result.printErrors()
        run = result.testsRun
        self.stream.write(TextTestResult.separator2 + "\n")
        self.stream.write(
            "Ran {} test{} in {:.3f}s\n\n".format(run, "" if run == 1 else "s", elapsed)
        )
        infos = []
        if not result.wasSuccessful():
            self.stream.write("FAILED")
            if result.failures:
                infos.append("failures={}".format(len(result.failures)))
            if result.errors:
                infos.append("errors={}".format(len(result.errors)))
            if result.unexpectedSuccesses:
                infos.append("unexpected successes={}".format(len(result.unexpectedSuccesses)))
        else:
            self.stream.write("OK")
        if result.skipped:
            infos.append("skipped={}".format(len(result.skipped)))
        if result.expectedFailures:
            infos.append("expected failures={}".format(len(result.expectedFailures)))
        if infos:
            self.stream.write(" ({})\n".format(", ".join(infos)))
        else:
            self.stream.write("\n")
        return result


class TestProgram:
    def __init__(
        self,
        module="__main__",
        defaultTest=None,
        argv=None,
        testRunner=None,
        testLoader=defaultTestLoader,
        exit=True,
        verbosity=1,
        failfast=None,
        catchbreak=None,
        buffer=None,
        warnings=None,
    ):
        if isinstance(module, str):
            self.module = sys.modules.get(module)
        else:
            self.module = module
        self.testLoader = testLoader
        self.verbosity = verbosity
        self.failfast = bool(failfast)
        self.buffer = bool(buffer)
        self.warnings = warnings
        self.exit = exit
        self.testRunner = testRunner
        args = list(sys.argv[1:]) if argv is None else list(argv[1:])
        test_names = []
        for arg in args:
            if arg in ("-v", "--verbose"):
                self.verbosity = 2
            elif arg in ("-q", "--quiet"):
                self.verbosity = 0
            elif arg in ("-f", "--failfast"):
                self.failfast = True
            elif not arg.startswith("-"):
                test_names.append(arg)
        if test_names:
            self.test = self.testLoader.loadTestsFromNames(test_names, self.module)
        elif defaultTest is not None:
            if isinstance(defaultTest, str):
                self.test = self.testLoader.loadTestsFromName(defaultTest, self.module)
            else:
                self.test = self.testLoader.loadTestsFromNames(defaultTest, self.module)
        elif self.module is not None:
            self.test = self.testLoader.loadTestsFromModule(self.module)
        else:
            self.test = TestSuite()
        self.runTests()

    def runTests(self):
        runner = self.testRunner
        if runner is None:
            runner = TextTestRunner(
                verbosity=self.verbosity,
                failfast=self.failfast,
                buffer=self.buffer,
                warnings=self.warnings,
            )
        elif isinstance(runner, type):
            runner = runner(verbosity=self.verbosity)
        self.result = runner.run(self.test)
        if self.exit:
            sys.exit(0 if self.result.wasSuccessful() else 1)


main = TestProgram
