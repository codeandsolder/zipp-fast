"""Single-threaded synchronization primitives and Thread stubs (`threading`) for Zipp Python."""

TIMEOUT_MAX = 9223372036.0


class Lock:
    def __init__(self):
        self._locked = False

    def acquire(self, blocking=True, timeout=-1):
        if self._locked:
            return False
        self._locked = True
        return True

    def release(self):
        if not self._locked:
            raise RuntimeError("release unlocked lock")
        self._locked = False

    def locked(self):
        return self._locked

    def __enter__(self):
        self.acquire()
        return True

    def __exit__(self, *exc):
        self.release()
        return False


class RLock:
    def __init__(self):
        self._count = 0

    def acquire(self, blocking=True, timeout=-1):
        self._count += 1
        return True

    def release(self):
        if self._count <= 0:
            raise RuntimeError("cannot release un-acquired lock")
        self._count -= 1

    def __enter__(self):
        self.acquire()
        return True

    def __exit__(self, *exc):
        self.release()
        return False


class Semaphore:
    def __init__(self, value=1):
        if value < 0:
            raise ValueError("semaphore initial value must be >= 0")
        self._value = value

    def acquire(self, blocking=True, timeout=None):
        if self._value <= 0:
            return False
        self._value -= 1
        return True

    def release(self, n=1):
        if n < 1:
            raise ValueError("n must be one or more")
        self._value += n

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, *exc):
        self.release()
        return False


class BoundedSemaphore(Semaphore):
    def __init__(self, value=1):
        super().__init__(value)
        self._initial_value = value

    def release(self, n=1):
        if self._value + n > self._initial_value:
            raise ValueError("Semaphore released too many times")
        super().release(n)


class Event:
    def __init__(self):
        self._flag = False

    def is_set(self):
        return self._flag

    isSet = is_set

    def set(self):
        self._flag = True

    def clear(self):
        self._flag = False

    def wait(self, timeout=None):
        return self._flag


class Condition:
    def __init__(self, lock=None):
        self._lock = RLock() if lock is None else lock
        self.acquire = self._lock.acquire
        self.release = self._lock.release

    def __enter__(self):
        return self._lock.__enter__()

    def __exit__(self, *args):
        return self._lock.__exit__(*args)

    def wait(self, timeout=None):
        return False

    def wait_for(self, predicate, timeout=None):
        return predicate()

    def notify(self, n=1):
        pass

    def notify_all(self):
        pass

    notifyAll = notify_all


class local:
    pass


class Thread:
    def __init__(self, group=None, target=None, name=None, args=(), kwargs=None, *, daemon=None):
        self._target = target
        self._args = args
        self._kwargs = {} if kwargs is None else dict(kwargs)
        self.name = name or "Thread"
        self.daemon = bool(daemon)
        self.ident = 1
        self._alive = False

    def run(self):
        if self._target is not None:
            self._target(*self._args, **self._kwargs)

    def start(self):
        self._alive = True
        try:
            self.run()
        finally:
            self._alive = False

    def join(self, timeout=None):
        pass

    def is_alive(self):
        return self._alive


_main_thread = Thread(name="MainThread")
_main_thread._alive = True


def current_thread():
    return _main_thread


currentThread = current_thread


def main_thread():
    return _main_thread


def get_ident():
    return 1


def get_native_id():
    return 1


def active_count():
    return 1


activeCount = active_count


def enumerate():
    return [_main_thread]
