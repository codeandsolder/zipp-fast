/* ZIPP Rust runtime — program entry, async main, test runner, and HTTP worker dispatch. Apache-2.0. */
function __zipp_rs_exit_status() { return __zipp_rs.__rt.exitStatus(); }
function __zipp_rs_run_tests() { return __zipp_rs.__rt.runTests(); }
function __zipp_rs_worker_request(reqJson) { return __zipp_rs.__rt.dispatchWorkerRequest(reqJson); }
(function (R) {
    "use strict";
    const rt = R.__rt;
    let exitCode = null;
    let mainFn = null;

    rt.exitStatus = function () { return exitCode; };
    R.setMain = function (fn) { mainFn = fn; };

    function handleMainResult(res) {
        const v = rt.deref(res);
        if (v && typeof v === "object" && v.__rs === 2 && v.__enum === "Result" && v.__var === "Err") {
            rt.writeStderr("Error: " + rt.fmtDebug(v.__data[0]) + "\n");
            exitCode = 1;
        }
    }

    function handleFatal(e) {
        if (e && e.isRsExit) {
            exitCode = e.code;
            return;
        }
        if (e && e.isRsPanic) {
            const text = "thread 'main' panicked at: " + e.message;
            rt.writeStderr(text + "\n");
            exitCode = 101;
            const err = new Error(text);
            err.name = "RustPanic";
            throw err;
        }
        const text = "runtime error: " + String(e);
        rt.writeStderr(text + "\n");
        exitCode = 101;
        throw e;
    }

    rt.runEntry = function (initFn) {
        try {
            initFn();
            // If running in --test or --serve worker boot mode, skip auto-calling `main()` if host requests!
            const mode = (typeof __zipp_rs_io === "function") ? __zipp_rs_io("mode", "") : "run";
            if (mode === "test") {
                return rt.runTests();
            }
            if (mode === "worker_boot") {
                // In worker boot mode, user can either expose `fn router() -> Router` (sync or async!) or run `main()`!
                rt.__inWorkerBoot = true;
                rt.__workerBootReady = false;
                try {
                    if (typeof R.getConst("__rs_fn_router") === "function") {
                        const r = R.blockOn(R.getConst("__rs_fn_router")());
                        if (r && r.__rs === 15 && r.kind === "router") rt.setRouter(r);
                    } else if (mainFn) {
                        const r = R.blockOn(mainFn());
                        if (r && r.__rs === 15 && r.kind === "router") rt.setRouter(r);
                        else handleMainResult(r);
                    }
                } finally {
                    rt.__inWorkerBoot = false;
                }
                return null;
            }
            if (mainFn) {
                const res = mainFn();
                if (res && typeof res.then === "function") {
                    return res.then(handleMainResult, handleFatal);
                }
                handleMainResult(res);
            }
            return null;
        } catch (e) {
            handleFatal(e);
            return null;
        }
    };

    rt.runTests = function () {
        const list = rt.tests;
        rt.writeStdout("\nrunning " + list.length + " test" + (list.length === 1 ? "" : "s") + "\n");
        let passed = 0, failed = 0, ignored = 0;
        const failures = [];
        for (let i = 0; i < list.length; i++) {
            const t = list[i];
            if (t.ignore) {
                ignored++;
                rt.writeStdout("test " + t.name + " ... ignored\n");
                continue;
            }
            try {
                const res = R.blockOn(t.fn());
                if (res && res.__rs === 2 && res.__enum === "Result" && res.__var === "Err") {
                    throw new rt.RsPanic("test returned Err(" + rt.fmtDebug(res.__data[0]) + ")");
                }
                if (t.shouldPanic) {
                    failed++;
                    failures.push([t.name, "note: test did not panic as expected"]);
                    rt.writeStdout("test " + t.name + " ... FAILED\n");
                } else {
                    passed++;
                    rt.writeStdout("test " + t.name + " ... ok\n");
                }
            } catch (e) {
                if (t.shouldPanic && e && e.isRsPanic) {
                    passed++;
                    rt.writeStdout("test " + t.name + " ... ok\n");
                } else {
                    failed++;
                    const msg = e && e.isRsPanic ? e.message : String(e);
                    failures.push([t.name, msg]);
                    rt.writeStdout("test " + t.name + " ... FAILED\n");
                }
            }
        }
        if (failures.length > 0) {
            rt.writeStdout("\nfailures:\n");
            for (let i = 0; i < failures.length; i++) {
                rt.writeStdout("\n---- " + failures[i][0] + " ----\n" + failures[i][1] + "\n");
            }
        }
        const statusStr = failed === 0 ? "ok" : "FAILED";
        rt.writeStdout("\ntest result: " + statusStr + ". " + passed + " passed; " + failed + " failed; " + ignored + " ignored\n\n");
        exitCode = failed === 0 ? 0 : 101;
        return failed === 0;
    };

    function pumpTasks() {
        for (let i = 0; i < 8; i++) {
            const fired = typeof rt.pollTimers === "function" ? rt.pollTimers() : 0;
            if (typeof __zipp_rs_io === "function") __zipp_rs_io("drain_microtasks", "");
            if (!fired) break;
        }
    }

    rt.dispatchWorkerRequest = function (reqJson) {
        const router = rt.getRouter();
        if (!router) {
            return JSON.stringify({
                status: 500,
                headers: [["Content-Type", "text/plain; charset=utf-8"]],
                body: "Rust worker script did not create a Router (return Router from `fn router()` or call `Router::new()...register()`)"
            });
        }
        let parsed = {};
        try { parsed = JSON.parse(reqJson); } catch (_) {}
        try {
            if (parsed && typeof parsed.__control === "string") {
                const ctrl = parsed.__control;
                if (ctrl === "poll_timers") {
                    pumpTasks();
                    return JSON.stringify({
                        ok: true,
                        wsFrames: typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [],
                        nextTimerMs: typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null
                    });
                }
                if (ctrl === "ws_open") {
                    const wsId = String(parsed.wsId || "");
                    const hdrs = Array.isArray(parsed.headers) ? parsed.headers.slice() : [];
                    hdrs.push(["Upgrade", "websocket"]);
                    hdrs.push(["Connection", "Upgrade"]);
                    const req = rt.mkRequest("GET", parsed.uri || "/", hdrs, "", null);
                    req.__wsId = wsId;
                    if (typeof rt.mkSocketAddr === "function") {
                        req.peerAddr = rt.mkSocketAddr("127.0.0.1", 0);
                    }
                    const resp = rt.routerDispatch(router, req);
                    if (resp && resp.__wsUpgraded && typeof rt.wsStartUpgrade === "function") {
                        rt.wsStartUpgrade(resp.__wsUpgraded);
                        pumpTasks();
                        return JSON.stringify({
                            ok: true,
                            accepted: true,
                            status: 101,
                            wsFrames: typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [],
                            nextTimerMs: typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null
                        });
                    }
                    pumpTasks();
                    return JSON.stringify({
                        ok: false,
                        accepted: false,
                        status: resp ? resp.status : 501,
                        body: resp ? (resp.body || "") : "WebSocket upgrade rejected",
                        wsFrames: typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [],
                        nextTimerMs: typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null
                    });
                }
                if (ctrl === "ws_send") {
                    const delivered = typeof rt.wsReceiveFromClient === "function"
                        ? rt.wsReceiveFromClient(parsed.wsId, parsed.data, !!parsed.binary)
                        : false;
                    pumpTasks();
                    return JSON.stringify({
                        ok: delivered,
                        wsFrames: typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [],
                        nextTimerMs: typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null
                    });
                }
                if (ctrl === "ws_close") {
                    const closed = typeof rt.wsCloseFromClient === "function"
                        ? rt.wsCloseFromClient(parsed.wsId, parsed.code, parsed.reason)
                        : false;
                    pumpTasks();
                    return JSON.stringify({
                        ok: closed,
                        wsFrames: typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [],
                        nextTimerMs: typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null
                    });
                }
            }
            if (typeof rt.pollTimers === "function") rt.pollTimers();
            const req = rt.mkRequest(parsed.method, parsed.uri, parsed.headers, parsed.body, null);
            if (typeof rt.mkSocketAddr === "function") {
                req.peerAddr = rt.mkSocketAddr("127.0.0.1", 0);
            }
            const resp = rt.routerDispatch(router, req);
            pumpTasks();
            const wsFrames = typeof rt.wsDrainOutbox === "function" ? rt.wsDrainOutbox() : [];
            const nextTimerMs = typeof rt.nextTimerMs === "function" ? rt.nextTimerMs() : null;
            if (wsFrames.length === 0 && nextTimerMs === null) {
                return rt.responseToJson(resp);
            }
            const hdrs = [];
            const entries = rt.mapEntries(resp.headers);
            for (let i = 0; i < entries.length; i++) {
                hdrs.push([rt.strVal(entries[i][0]), rt.strVal(entries[i][1])]);
            }
            const out = {
                status: resp.status,
                headers: hdrs,
                body: resp.body
            };
            if (wsFrames.length > 0) out.wsFrames = wsFrames;
            if (nextTimerMs !== null) out.nextTimerMs = nextTimerMs;
            return JSON.stringify(out);
        } catch (e) {
            const msg = e && e.isRsPanic ? ("Rust panic: " + e.message) : String(e);
            return JSON.stringify({
                status: 500,
                headers: [["Content-Type", "text/plain; charset=utf-8"]],
                body: msg
            });
        }
    };
})(__zipp_rs);
function __zipp_rs_entry() {}
__zipp_rs.__rt.runEntry(__zipp_rs_entry);
