"""PEP 492/525/530 async/await, async for/with/generators/comprehensions, and asyncio."""
import asyncio


# ---- Basic coroutines & custom __await__ -------------------------------------
async def add_one(x):
    await asyncio.sleep(0)
    return x + 1


class CustomAwaitable:
    def __init__(self, val):
        self.val = val

    def __await__(self):
        yield
        return self.val * 10


async def generic_async[T](x: T) -> T:
    await asyncio.sleep(0)
    return x


# ---- Async iterator & async generator ---------------------------------------
class AsyncRange:
    def __init__(self, n):
        self.i = 0
        self.n = n

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.i >= self.n:
            raise StopAsyncIteration
        v = self.i
        self.i += 1
        await asyncio.sleep(0)
        return v


async def agen_squares(n):
    for i in range(n):
        await asyncio.sleep(0)
        yield i * i


async def agen_echo():
    log = []
    try:
        v = yield "ready"
        while v is not None:
            log.append(v)
            await asyncio.sleep(0)
            v = yield f"got:{v}"
    finally:
        echo_closed.append(tuple(log))


echo_closed = []


# ---- Async context manager ---------------------------------------------------
class AsyncCtx:
    def __init__(self, name, suppress=False):
        self.name = name
        self.suppress = suppress

    async def __aenter__(self):
        ctx_log.append(f"enter:{self.name}")
        await asyncio.sleep(0)
        return self.name.upper()

    async def __aexit__(self, exc_type, exc, tb):
        await asyncio.sleep(0)
        ctx_log.append(f"exit:{self.name}:{exc_type.__name__ if exc_type else None}")
        return self.suppress


ctx_log = []

# Async genexpr created OUTSIDE an async function (PEP 530).
outer_agen = (x * 3 async for x in AsyncRange(4) if x % 2 == 1)


async def main():
    # Basic await + custom awaitable + generic async function
    r1 = await add_one(41)
    r2 = await CustomAwaitable(7)
    r3 = await generic_async("ok")
    print("basic", r1, r2, r3, generic_async.__type_params__[0].__name__)
    print("introspect", asyncio.iscoroutinefunction(add_one), asyncio.iscoroutinefunction(lambda: 1))

    # aiter / anext with and without default
    it = aiter(AsyncRange(2))
    print("anext", await anext(it), await anext(it), await anext(it, "done"))

    # async for + else + break + continue
    vals = []
    async for x in AsyncRange(5):
        if x == 2:
            continue
        vals.append(x)
    else:
        vals.append("else")
    vals2 = []
    async for x in AsyncRange(5):
        if x == 2:
            break
        vals2.append(x)
    else:
        vals2.append("nope")
    print("async_for", vals, vals2)

    # async generator + asend + aclose
    sq = [x async for x in agen_squares(5)]
    print("agen_sq", sq)
    eg = agen_echo()
    first = await anext(eg)
    second = await eg.asend("a")
    third = await eg.asend("b")
    await eg.aclose()
    print("agen_echo", first, second, third, echo_closed)

    # async comprehensions (list, set, dict, genexpr)
    lc = [await add_one(x) for x in range(4)]
    sc = sorted({x * 2 async for x in AsyncRange(4)})
    dc = {x: await add_one(x) async for x in AsyncRange(3)}
    gc = [x async for x in outer_agen]
    print("comps", lc, sc, dc, gc)

    # async with (normal, multi-item, suppress, propagate)
    async with AsyncCtx("a") as a, AsyncCtx("b") as b:
        ctx_log.append(f"body:{a}:{b}")
    async with AsyncCtx("s", suppress=True):
        raise ValueError("suppressed")
    try:
        async with AsyncCtx("p", suppress=False):
            raise KeyError("boom")
    except KeyError:
        ctx_log.append("caught:KeyError")
    print("async_with", ctx_log)

    # asyncio.gather (normal & return_exceptions)
    g1 = await asyncio.gather(add_one(1), add_one(2), add_one(3))
    async def fail_coro():
        await asyncio.sleep(0)
        raise ValueError("bad")
    g2 = await asyncio.gather(add_one(10), fail_coro(), return_exceptions=True)
    print("gather", g1, g2[0], type(g2[1]).__name__, str(g2[1]))

    # create_task, cancellation, shield
    cancel_log = []
    async def cancellable():
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            cancel_log.append("cancelled")
            raise
    t = asyncio.create_task(cancellable(), name="worker")
    await asyncio.sleep(0)
    t.cancel()
    try:
        await t
    except asyncio.CancelledError:
        cancel_log.append("awaited_cancel")
    print("cancel", t.get_name(), t.cancelled(), cancel_log)

    # wait_for & timeout
    ok_wf = await asyncio.wait_for(add_one(5), timeout=1.0)
    timed = False
    try:
        await asyncio.wait_for(asyncio.sleep(1.0), timeout=0.005)
    except TimeoutError:
        timed = True
    timed2 = False
    try:
        async with asyncio.timeout(0.005):
            await asyncio.sleep(1.0)
    except TimeoutError:
        timed2 = True
    print("timeouts", ok_wf, timed, timed2)

    # wait (FIRST_COMPLETED & ALL_COMPLETED)
    t1 = asyncio.create_task(add_one(1))
    t2 = asyncio.create_task(add_one(2))
    done, pending = await asyncio.wait([t1, t2], return_when=asyncio.ALL_COMPLETED)
    print("wait_all", sorted(f.result() for f in done), len(pending))

    # Event, Lock, Semaphore, BoundedSemaphore
    ev = asyncio.Event()
    ev_log = []
    async def waiter():
        ev_log.append("waiting")
        await ev.wait()
        ev_log.append("woke")
    wt = asyncio.create_task(waiter())
    await asyncio.sleep(0)
    ev.set()
    await wt
    print("event", ev.is_set(), ev_log)

    lock = asyncio.Lock()
    lock_log = []
    async def crit(tag):
        async with lock:
            lock_log.append(f"in:{tag}")
            await asyncio.sleep(0)
            lock_log.append(f"out:{tag}")
    await asyncio.gather(crit("A"), crit("B"))
    print("lock", lock_log)

    sem = asyncio.BoundedSemaphore(2)
    async with sem:
        async with sem:
            sem_locked = sem.locked()
    print("sem", sem_locked, sem.locked())

    # Queue producer/consumer + LifoQueue + PriorityQueue
    q = asyncio.Queue(maxsize=2)
    out = []
    async def producer():
        for i in range(4):
            await q.put(i)
    async def consumer():
        for _ in range(4):
            item = await q.get()
            out.append(item)
            q.task_done()
    await asyncio.gather(producer(), consumer())
    await q.join()
    lq = asyncio.LifoQueue()
    lq.put_nowait("a")
    lq.put_nowait("b")
    pq = asyncio.PriorityQueue()
    pq.put_nowait((2, "two"))
    pq.put_nowait((1, "one"))
    print("queues", out, lq.get_nowait(), lq.get_nowait(), pq.get_nowait(), pq.get_nowait())

    # Barrier
    bar = asyncio.Barrier(3)
    async def barr_worker(i):
        await asyncio.sleep(0)
        return await bar.wait()
    b_res = await asyncio.gather(barr_worker(0), barr_worker(1), barr_worker(2))
    print("barrier", sorted(b_res))

    # TaskGroup (success & structured error handling with except*)
    async with asyncio.TaskGroup() as tg:
        ta = tg.create_task(add_one(10))
        tb = tg.create_task(add_one(20))
    print("tg_ok", ta.result(), tb.result())

    tg_errs = []
    try:
        async with asyncio.TaskGroup() as tg:
            async def boom(cls, msg):
                await asyncio.sleep(0)
                raise cls(msg)
            tg.create_task(boom(ValueError, "v"))
            tg.create_task(boom(KeyError, "k"))
    except* ValueError as eg:
        tg_errs.append(("V", [str(x) for x in eg.exceptions]))
    except* KeyError as eg:
        tg_errs.append(("K", [str(x) for x in eg.exceptions]))
    print("tg_err", sorted(tg_errs))


asyncio.run(main())
