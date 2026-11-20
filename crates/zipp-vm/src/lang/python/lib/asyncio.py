"""Cooperative asyncio event loop, tasks, synchronization primitives, and TaskGroup."""

import collections
import heapq
import time

FIRST_COMPLETED = "FIRST_COMPLETED"
FIRST_EXCEPTION = "FIRST_EXCEPTION"
ALL_COMPLETED = "ALL_COMPLETED"

TimeoutError = TimeoutError


class CancelledError(BaseException):
    pass


class InvalidStateError(Exception):
    pass


class QueueEmpty(Exception):
    pass


class QueueFull(Exception):
    pass


class BrokenBarrierError(RuntimeError):
    pass


_RUNNING_LOOP = None
_DEFAULT_LOOP = None
_CURRENT_TASK = None
_TASK_COUNTER = 0


def iscoroutine(obj):
    return type(obj).__name__ == "coroutine"


def iscoroutinefunction(func):
    code = getattr(func, "__code__", None)
    if code is None:
        return False
    return bool(getattr(code, "co_flags", 0) & 0x80)


def isfuture(obj):
    return isinstance(obj, Future) or getattr(obj, "_asyncio_future_blocking", None) is not None


class Handle:
    def __init__(self, callback, args, loop):
        self._callback = callback
        self._args = args
        self._loop = loop
        self._cancelled = False

    def cancel(self):
        self._cancelled = True

    def cancelled(self):
        return self._cancelled

    def _run(self):
        if not self._cancelled:
            self._callback(*self._args)


class TimerHandle(Handle):
    def __init__(self, when, callback, args, loop):
        super().__init__(callback, args, loop)
        self._when = when

    def when(self):
        return self._when


class _Sleep0:
    def __await__(self):
        yield


class Future:
    _asyncio_future_blocking = False

    def __init__(self, *, loop=None):
        self._loop = loop if loop is not None else get_running_loop()
        self._state = "PENDING"
        self._result = None
        self._exception = None
        self._cancel_message = None
        self._callbacks = []

    def get_loop(self):
        return self._loop

    def cancelled(self):
        return self._state == "CANCELLED"

    def done(self):
        return self._state != "PENDING"

    def result(self):
        if self._state == "CANCELLED":
            if self._cancel_message is not None:
                raise CancelledError(self._cancel_message)
            raise CancelledError()
        if self._state != "FINISHED":
            raise InvalidStateError("Result is not set.")
        if self._exception is not None:
            raise self._exception
        return self._result

    def exception(self):
        if self._state == "CANCELLED":
            if self._cancel_message is not None:
                raise CancelledError(self._cancel_message)
            raise CancelledError()
        if self._state != "FINISHED":
            raise InvalidStateError("Exception is not set.")
        return self._exception

    def add_done_callback(self, fn):
        if self._state != "PENDING":
            self._loop.call_soon(fn, self)
        else:
            self._callbacks.append(fn)

    def remove_done_callback(self, fn):
        kept = [cb for cb in self._callbacks if cb != fn]
        removed = len(self._callbacks) - len(kept)
        self._callbacks = kept
        return removed

    def _schedule_callbacks(self):
        cbs = self._callbacks
        if not cbs:
            return
        self._callbacks = []
        for cb in cbs:
            self._loop.call_soon(cb, self)

    def set_result(self, result):
        if self._state != "PENDING":
            raise InvalidStateError("Future is already done.")
        self._result = result
        self._state = "FINISHED"
        self._schedule_callbacks()

    def set_exception(self, exception):
        if self._state != "PENDING":
            raise InvalidStateError("Future is already done.")
        if isinstance(exception, type):
            exception = exception()
        self._exception = exception
        self._state = "FINISHED"
        self._schedule_callbacks()

    def cancel(self, msg=None):
        if self._state != "PENDING":
            return False
        self._state = "CANCELLED"
        self._cancel_message = msg
        self._schedule_callbacks()
        return True

    def __await__(self):
        if not self.done():
            self._asyncio_future_blocking = True
            yield self
        if not self.done():
            raise RuntimeError("await wasn't used with future")
        return self.result()

    __iter__ = __await__


class Task(Future):
    def __init__(self, coro, *, loop=None, name=None):
        super().__init__(loop=loop)
        global _TASK_COUNTER
        _TASK_COUNTER += 1
        self._coro = coro
        self._name = name if name is not None else f"Task-{_TASK_COUNTER}"
        self._fut_waiter = None
        self._must_cancel = False
        self._cancel_msg = None
        self._num_cancels_requested = 0
        self._loop._tasks.add(self)
        self._loop.call_soon(self._step)

    def get_name(self):
        return self._name

    def set_name(self, value):
        self._name = str(value)

    def get_coro(self):
        return self._coro

    def cancelling(self):
        return self._num_cancels_requested

    def uncancel(self):
        if self._num_cancels_requested > 0:
            self._num_cancels_requested -= 1
        return self._num_cancels_requested

    def cancel(self, msg=None):
        if self.done():
            return False
        self._num_cancels_requested += 1
        if self._fut_waiter is not None:
            if self._fut_waiter.cancel(msg=msg):
                return True
        self._must_cancel = True
        self._cancel_msg = msg
        return True

    def _step(self, exc=None):
        if self.done():
            return
        if self._must_cancel:
            if not isinstance(exc, CancelledError):
                exc = CancelledError(self._cancel_msg) if self._cancel_msg is not None else CancelledError()
            self._must_cancel = False
            self._cancel_msg = None
        self._fut_waiter = None
        global _CURRENT_TASK
        prev_task = _CURRENT_TASK
        _CURRENT_TASK = self
        try:
            if exc is None:
                result = self._coro.send(None)
            else:
                result = self._coro.throw(exc)
        except StopIteration as stop:
            self._loop._tasks.discard(self)
            if self._must_cancel:
                self._must_cancel = False
                super().cancel(msg=self._cancel_msg)
            else:
                super().set_result(stop.value)
        except CancelledError as ce:
            self._loop._tasks.discard(self)
            msg = ce.args[0] if ce.args else None
            super().cancel(msg=msg)
        except BaseException as err:
            self._loop._tasks.discard(self)
            super().set_exception(err)
        else:
            blocking = getattr(result, "_asyncio_future_blocking", None)
            if blocking is not None:
                if blocking:
                    result._asyncio_future_blocking = False
                    if result is self:
                        self._loop.call_soon(self._step, RuntimeError("Task cannot await on itself"))
                    else:
                        result.add_done_callback(self._wakeup)
                        self._fut_waiter = result
                        if self._must_cancel:
                            if self._fut_waiter.cancel(msg=self._cancel_msg):
                                self._must_cancel = False
                else:
                    self._loop.call_soon(self._step, RuntimeError("yield was used instead of yield from"))
            elif result is None:
                self._loop.call_soon(self._step)
            else:
                self._loop.call_soon(self._step, RuntimeError(f"Task got bad yield: {result!r}"))
        finally:
            _CURRENT_TASK = prev_task

    def _wakeup(self, future):
        try:
            future.result()
        except BaseException as exc:
            self._step(exc)
        else:
            self._step()


class EventLoop:
    def __init__(self):
        self._ready = collections.deque()
        self._scheduled = []
        self._seq = 0
        self._tasks = set()
        self._running = False
        self._stopping = False
        self._closed = False
        self._clock_offset = 0.0

    def time(self):
        return time.monotonic() + self._clock_offset

    def is_running(self):
        return self._running

    def is_closed(self):
        return self._closed

    def create_future(self):
        return Future(loop=self)

    def create_task(self, coro, *, name=None):
        if self._closed:
            raise RuntimeError("Event loop is closed")
        return Task(coro, loop=self, name=name)

    def call_soon(self, callback, *args):
        h = Handle(callback, args, self)
        self._ready.append(h)
        return h

    def call_later(self, delay, callback, *args):
        return self.call_at(self.time() + delay, callback, *args)

    def call_at(self, when, callback, *args):
        h = TimerHandle(when, callback, args, self)
        self._seq += 1
        heapq.heappush(self._scheduled, (when, self._seq, h))
        return h

    def stop(self):
        self._stopping = True

    def close(self):
        if self._running:
            raise RuntimeError("Cannot close a running event loop")
        self._ready.clear()
        self._scheduled.clear()
        self._closed = True

    def _run_once(self):
        while self._scheduled and self._scheduled[0][2]._cancelled:
            heapq.heappop(self._scheduled)
        if not self._ready and self._scheduled:
            when = self._scheduled[0][0]
            now = self.time()
            dt = when - now
            if dt > 0.0:
                time.sleep(dt)
                after = self.time()
                if after < when:
                    self._clock_offset += when - after
        now = self.time()
        while self._scheduled and self._scheduled[0][0] <= now:
            _, _, h = heapq.heappop(self._scheduled)
            if not h._cancelled:
                self._ready.append(h)
        ntodo = len(self._ready)
        for _ in range(ntodo):
            h = self._ready.popleft()
            if not h._cancelled:
                h._run()

    def run_forever(self):
        if self._closed:
            raise RuntimeError("Event loop is closed")
        if self._running:
            raise RuntimeError("This event loop is already running")
        global _RUNNING_LOOP
        prev = _RUNNING_LOOP
        _RUNNING_LOOP = self
        self._running = True
        self._stopping = False
        try:
            while not self._stopping:
                if not self._ready and not self._scheduled:
                    break
                self._run_once()
        finally:
            self._running = False
            self._stopping = False
            _RUNNING_LOOP = prev

    def run_until_complete(self, future):
        if self._closed:
            raise RuntimeError("Event loop is closed")
        fut = ensure_future(future, loop=self)
        fut.add_done_callback(lambda _f: self.stop())
        self.run_forever()
        if not fut.done():
            raise RuntimeError("Event loop stopped before Future completed.")
        return fut.result()


def new_event_loop():
    return EventLoop()


def set_event_loop(loop):
    global _DEFAULT_LOOP
    _DEFAULT_LOOP = loop


def get_running_loop():
    if _RUNNING_LOOP is None:
        raise RuntimeError("no running event loop")
    return _RUNNING_LOOP


def get_event_loop():
    if _RUNNING_LOOP is not None:
        return _RUNNING_LOOP
    global _DEFAULT_LOOP
    if _DEFAULT_LOOP is None or _DEFAULT_LOOP.is_closed():
        _DEFAULT_LOOP = EventLoop()
    return _DEFAULT_LOOP


def current_task(loop=None):
    return _CURRENT_TASK


def all_tasks(loop=None):
    if loop is None:
        loop = get_running_loop()
    return {t for t in loop._tasks if not t.done()}


def create_task(coro, *, name=None):
    return get_running_loop().create_task(coro, name=name)


def ensure_future(coro_or_future, *, loop=None):
    if isfuture(coro_or_future):
        return coro_or_future
    if iscoroutine(coro_or_future):
        if loop is None:
            loop = get_running_loop()
        return loop.create_task(coro_or_future)
    if hasattr(coro_or_future, "__await__"):
        async def _wrap(aw):
            return await aw
        if loop is None:
            loop = get_running_loop()
        return loop.create_task(_wrap(coro_or_future))
    raise TypeError("An asyncio.Future, a coroutine or an awaitable is required")


def run(main):
    if _RUNNING_LOOP is not None:
        raise RuntimeError("asyncio.run() cannot be called from a running event loop")
    if not iscoroutine(main):
        raise ValueError(f"a coroutine was expected, got {main!r}")
    loop = EventLoop()
    try:
        return loop.run_until_complete(main)
    finally:
        pending = list(loop._tasks)
        for t in pending:
            t.cancel()
        if pending:
            async def _drain():
                for t in pending:
                    try:
                        await t
                    except BaseException:
                        pass
            loop.run_until_complete(_drain())
        loop.close()


async def sleep(delay, result=None):
    if delay <= 0:
        await _Sleep0()
        return result
    loop = get_running_loop()
    fut = loop.create_future()
    def _done():
        if not fut.done():
            fut.set_result(result)
    h = loop.call_later(delay, _done)
    try:
        return await fut
    finally:
        h.cancel()


async def gather(*coros_or_futures, return_exceptions=False):
    if not coros_or_futures:
        return []
    loop = get_running_loop()
    futs = [ensure_future(c, loop=loop) for c in coros_or_futures]
    n = len(futs)
    results = [None] * n
    remaining = [n]
    outer = loop.create_future()

    def _make_cb(idx):
        def _cb(f):
            if outer.done():
                return
            if f.cancelled():
                err = CancelledError()
                if return_exceptions:
                    results[idx] = err
                else:
                    outer.set_exception(err)
                    return
            else:
                exc = f.exception()
                if exc is not None:
                    if return_exceptions:
                        results[idx] = exc
                    else:
                        outer.set_exception(exc)
                        return
                else:
                    results[idx] = f.result()
            remaining[0] -= 1
            if remaining[0] == 0:
                outer.set_result(results)
        return _cb

    for i, f in enumerate(futs):
        f.add_done_callback(_make_cb(i))

    try:
        return await outer
    except CancelledError:
        for f in futs:
            f.cancel()
        raise


def shield(arg):
    loop = get_running_loop()
    inner = ensure_future(arg, loop=loop)
    if inner.done():
        return inner
    outer = loop.create_future()

    def _done(f):
        if outer.cancelled():
            return
        if f.cancelled():
            outer.cancel()
        else:
            exc = f.exception()
            if exc is not None:
                outer.set_exception(exc)
            else:
                outer.set_result(f.result())

    inner.add_done_callback(_done)
    return outer


async def wait_for(aw, timeout):
    loop = get_running_loop()
    if timeout is None:
        return await aw
    fut = ensure_future(aw, loop=loop)
    if timeout <= 0:
        if fut.done():
            return fut.result()
        fut.cancel()
        try:
            await fut
        except CancelledError:
            pass
        raise TimeoutError()
    waiter = loop.create_future()
    timed_out = [False]

    def _on_timeout():
        if not waiter.done():
            timed_out[0] = True
            fut.cancel()
            waiter.set_result(None)

    def _on_done(f):
        if not waiter.done():
            waiter.set_result(None)

    h = loop.call_later(timeout, _on_timeout)
    fut.add_done_callback(_on_done)
    try:
        await waiter
    except CancelledError:
        h.cancel()
        fut.cancel()
        raise
    finally:
        h.cancel()
    if timed_out[0]:
        try:
            await fut
        except CancelledError:
            pass
        raise TimeoutError()
    return fut.result()


async def wait(fs, *, timeout=None, return_when=ALL_COMPLETED):
    loop = get_running_loop()
    futs = {ensure_future(f, loop=loop) for f in fs}
    if not futs:
        return set(), set()
    if return_when not in (FIRST_COMPLETED, FIRST_EXCEPTION, ALL_COMPLETED):
        raise ValueError(f"Invalid return_when value: {return_when}")
    waiter = loop.create_future()
    counter = [len(futs)]

    def _on_done(f):
        counter[0] -= 1
        if waiter.done():
            return
        if return_when == FIRST_COMPLETED:
            waiter.set_result(None)
        elif return_when == FIRST_EXCEPTION:
            if (not f.cancelled() and f.exception() is not None) or counter[0] == 0:
                waiter.set_result(None)
        elif counter[0] == 0:
            waiter.set_result(None)

    for f in futs:
        f.add_done_callback(_on_done)
    timer = None
    if timeout is not None:
        timer = loop.call_later(timeout, lambda: waiter.set_result(None) if not waiter.done() else None)
    try:
        await waiter
    finally:
        if timer is not None:
            timer.cancel()
        for f in futs:
            f.remove_done_callback(_on_done)
    done = {f for f in futs if f.done()}
    pending = futs - done
    return done, pending


def as_completed(aws, *, timeout=None):
    loop = get_running_loop()
    futs = [ensure_future(a, loop=loop) for a in aws]
    q = Queue()
    timed_out = [False]
    timer = None

    def _on_done(f):
        q.put_nowait(f)

    for f in futs:
        f.add_done_callback(_on_done)
    if timeout is not None:
        def _on_timeout():
            timed_out[0] = True
            q.put_nowait(None)
        timer = loop.call_later(timeout, _on_timeout)

    async def _wait_one():
        f = await q.get()
        if f is None:
            raise TimeoutError()
        return f.result()

    out = []
    for _ in range(len(futs)):
        out.append(_wait_one())
    return out


class Timeout:
    def __init__(self, when):
        self._when = when
        self._expired = False
        self._timer = None
        self._task = None
        self._cancelling = 0

    def when(self):
        return self._when

    def expired(self):
        return self._expired

    def reschedule(self, when):
        if self._timer is not None:
            self._timer.cancel()
            self._timer = None
        self._when = when
        if when is not None and self._task is not None:
            loop = get_running_loop()
            if when <= loop.time():
                self._on_timeout()
            else:
                self._timer = loop.call_at(when, self._on_timeout)

    def _on_timeout(self):
        self._expired = True
        if self._task is not None:
            self._task.cancel()

    async def __aenter__(self):
        self._task = current_task()
        self._cancelling = self._task.cancelling() if self._task is not None else 0
        if self._when is not None:
            self.reschedule(self._when)
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self._timer is not None:
            self._timer.cancel()
            self._timer = None
        if self._expired and exc_type is not None and issubclass(exc_type, CancelledError):
            if self._task is not None and self._task.uncancel() <= self._cancelling:
                raise TimeoutError() from exc_val
        return False


def timeout(delay):
    when = None if delay is None else get_running_loop().time() + delay
    return Timeout(when)


def timeout_at(when):
    return Timeout(when)


class TaskGroup:
    def __init__(self):
        self._entered = False
        self._exiting = False
        self._aborting = False
        self._tasks = set()
        self._errors = []
        self._parent_task = None
        self._on_completed_fut = None

    async def __aenter__(self):
        if self._entered:
            raise RuntimeError("TaskGroup has already been entered")
        self._entered = True
        self._parent_task = current_task()
        return self

    def create_task(self, coro, *, name=None):
        if not self._entered:
            raise RuntimeError("TaskGroup has not been entered")
        if self._exiting and not self._tasks:
            raise RuntimeError("TaskGroup is finished")
        t = get_running_loop().create_task(coro, name=name)
        self._tasks.add(t)
        t.add_done_callback(self._on_task_done)
        return t

    def _abort(self):
        self._aborting = True
        for t in list(self._tasks):
            if not t.done():
                t.cancel()

    def _on_task_done(self, task):
        self._tasks.discard(task)
        if self._on_completed_fut is not None and not self._tasks:
            if not self._on_completed_fut.done():
                self._on_completed_fut.set_result(True)
        if task.cancelled():
            return
        exc = task.exception()
        if exc is not None:
            self._errors.append(exc)
            if not self._aborting:
                self._abort()

    async def __aexit__(self, et, exc, tb):
        self._exiting = True
        if exc is not None:
            if not self._aborting:
                self._abort()
            if not isinstance(exc, CancelledError):
                self._errors.append(exc)
        while self._tasks:
            self._on_completed_fut = get_running_loop().create_future()
            try:
                await self._on_completed_fut
            except CancelledError:
                if not self._aborting:
                    self._abort()
            self._on_completed_fut = None
        if self._errors:
            errs = self._errors
            self._errors = []
            raise BaseExceptionGroup("unhandled errors in a TaskGroup", errs)
        return False


class Event:
    def __init__(self):
        self._value = False
        self._waiters = collections.deque()

    def is_set(self):
        return self._value

    def set(self):
        if not self._value:
            self._value = True
            while self._waiters:
                fut = self._waiters.popleft()
                if not fut.done():
                    fut.set_result(True)

    def clear(self):
        self._value = False

    async def wait(self):
        if self._value:
            return True
        fut = get_running_loop().create_future()
        self._waiters.append(fut)
        try:
            await fut
            return True
        finally:
            if fut in self._waiters:
                self._waiters.remove(fut)


class Lock:
    def __init__(self):
        self._locked = False
        self._waiters = collections.deque()

    def locked(self):
        return self._locked

    async def acquire(self):
        if not self._locked and not self._waiters:
            self._locked = True
            return True
        fut = get_running_loop().create_future()
        self._waiters.append(fut)
        try:
            await fut
            self._locked = True
            return True
        except BaseException:
            if fut in self._waiters:
                self._waiters.remove(fut)
            if not self._locked:
                self._wake_up_first()
            raise

    def release(self):
        if not self._locked:
            raise RuntimeError("Lock is not acquired.")
        self._locked = False
        self._wake_up_first()

    def _wake_up_first(self):
        while self._waiters:
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(True)
                break

    async def __aenter__(self):
        await self.acquire()
        return None

    async def __aexit__(self, exc_type, exc, tb):
        self.release()
        return False


class Semaphore:
    def __init__(self, value=1):
        if value < 0:
            raise ValueError("Semaphore initial value must be >= 0")
        self._value = value
        self._waiters = collections.deque()

    def locked(self):
        return self._value == 0

    async def acquire(self):
        while self._value <= 0:
            fut = get_running_loop().create_future()
            self._waiters.append(fut)
            try:
                await fut
            except BaseException:
                if fut in self._waiters:
                    self._waiters.remove(fut)
                if self._value > 0:
                    self._wake_up_next()
                raise
        self._value -= 1
        return True

    def release(self):
        self._value += 1
        self._wake_up_next()

    def _wake_up_next(self):
        while self._waiters:
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(None)
                break

    async def __aenter__(self):
        await self.acquire()
        return None

    async def __aexit__(self, exc_type, exc, tb):
        self.release()
        return False


class BoundedSemaphore(Semaphore):
    def __init__(self, value=1):
        super().__init__(value)
        self._bound_value = value

    def release(self):
        if self._value >= self._bound_value:
            raise ValueError("BoundedSemaphore released too many times")
        super().release()


class Condition:
    def __init__(self, lock=None):
        self._lock = lock if lock is not None else Lock()
        self._waiters = collections.deque()

    def locked(self):
        return self._lock.locked()

    async def acquire(self):
        return await self._lock.acquire()

    def release(self):
        self._lock.release()

    async def __aenter__(self):
        await self._lock.acquire()
        return None

    async def __aexit__(self, exc_type, exc, tb):
        self._lock.release()
        return False

    async def wait(self):
        if not self.locked():
            raise RuntimeError("cannot wait on un-acquired lock")
        self.release()
        try:
            fut = get_running_loop().create_future()
            self._waiters.append(fut)
            try:
                await fut
                return True
            finally:
                if fut in self._waiters:
                    self._waiters.remove(fut)
        finally:
            await self.acquire()

    async def wait_for(self, predicate):
        result = predicate()
        while not result:
            await self.wait()
            result = predicate()
        return result

    def notify(self, n=1):
        if not self.locked():
            raise RuntimeError("cannot notify on un-acquired lock")
        idx = 0
        while idx < n and self._waiters:
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(False)
                idx += 1

    def notify_all(self):
        self.notify(len(self._waiters))


class Barrier:
    def __init__(self, parties):
        if parties < 1:
            raise ValueError("parties must be >= 1")
        self._parties = parties
        self._count = 0
        self._broken = False
        self._waiters = []

    @property
    def parties(self):
        return self._parties

    @property
    def n_waiting(self):
        return self._count

    @property
    def broken(self):
        return self._broken

    async def wait(self):
        if self._broken:
            raise BrokenBarrierError("Barrier is broken")
        idx = self._count
        self._count += 1
        if self._count == self._parties:
            self._count = 0
            ws = self._waiters
            self._waiters = []
            for fut in ws:
                if not fut.done():
                    fut.set_result(None)
            return idx
        fut = get_running_loop().create_future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException:
            if fut in self._waiters:
                self._waiters.remove(fut)
            self._broken = True
            for w in self._waiters:
                if not w.done():
                    w.set_exception(BrokenBarrierError("Barrier aborted"))
            self._waiters = []
            raise
        return idx

    async def abort(self):
        self._broken = True
        for w in self._waiters:
            if not w.done():
                w.set_exception(BrokenBarrierError("Barrier aborted"))
        self._waiters = []
        self._count = 0

    async def reset(self):
        await self.abort()
        self._broken = False


class Queue:
    def __init__(self, maxsize=0):
        self._maxsize = maxsize
        self._getters = collections.deque()
        self._putters = collections.deque()
        self._unfinished_tasks = 0
        self._finished = Event()
        self._finished.set()
        self._init(maxsize)

    def _init(self, maxsize):
        self._queue = collections.deque()

    def _get(self):
        return self._queue.popleft()

    def _put(self, item):
        self._queue.append(item)

    def qsize(self):
        return len(self._queue)

    @property
    def maxsize(self):
        return self._maxsize

    def empty(self):
        return not self._queue

    def full(self):
        return self._maxsize > 0 and self.qsize() >= self._maxsize

    def _wakeup_next(self, waiters):
        while waiters:
            w = waiters.popleft()
            if not w.done():
                w.set_result(None)
                break

    def put_nowait(self, item):
        if self.full():
            raise QueueFull()
        self._put(item)
        self._unfinished_tasks += 1
        self._finished.clear()
        self._wakeup_next(self._getters)

    async def put(self, item):
        while self.full():
            fut = get_running_loop().create_future()
            self._putters.append(fut)
            try:
                await fut
            except BaseException:
                if fut in self._putters:
                    self._putters.remove(fut)
                if not self.full():
                    self._wakeup_next(self._putters)
                raise
        self.put_nowait(item)

    def get_nowait(self):
        if self.empty():
            raise QueueEmpty()
        item = self._get()
        self._wakeup_next(self._putters)
        return item

    async def get(self):
        while self.empty():
            fut = get_running_loop().create_future()
            self._getters.append(fut)
            try:
                await fut
            except BaseException:
                if fut in self._getters:
                    self._getters.remove(fut)
                if not self.empty():
                    self._wakeup_next(self._getters)
                raise
        return self.get_nowait()

    def task_done(self):
        if self._unfinished_tasks <= 0:
            raise ValueError("task_done() called too many times")
        self._unfinished_tasks -= 1
        if self._unfinished_tasks == 0:
            self._finished.set()

    async def join(self):
        if self._unfinished_tasks > 0:
            await self._finished.wait()


class LifoQueue(Queue):
    def _init(self, maxsize):
        self._queue = []

    def _put(self, item):
        self._queue.append(item)

    def _get(self):
        return self._queue.pop()


class PriorityQueue(Queue):
    def _init(self, maxsize):
        self._queue = []

    def _put(self, item):
        heapq.heappush(self._queue, item)

    def _get(self):
        return heapq.heappop(self._queue)
