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
                if (typeof R.getConst("__rs_fn_router") === "function") {
                    const r = R.blockOn(R.getConst("__rs_fn_router")());
                    if (r && r.__rs === 15 && r.kind === "router") rt.setRouter(r);
                } else if (mainFn) {
                    const r = R.blockOn(mainFn());
                    if (r && r.__rs === 15 && r.kind === "router") rt.setRouter(r);
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
            const req = rt.mkRequest(parsed.method, parsed.uri, parsed.headers, parsed.body, null);
            const resp = rt.routerDispatch(router, req);
            return rt.responseToJson(resp);
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
