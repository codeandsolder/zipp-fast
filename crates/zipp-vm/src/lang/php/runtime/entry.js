/* ZIPP PHP runtime — host hooks & program entry. Apache-2.0. */
function __zipp_php_exit_status() { return __zipp_php.__rt.exitStatus(); }
function __zipp_php_take_response() { return __zipp_php.__rt.takeSapiResponse(); }
function __zipp_php_worker_request(reqJson) { return __zipp_php.__rt.dispatchWorkerRequest(reqJson); }
(function (R) {
    "use strict";
    const rt = R.__rt;
    let exitCode = null;

    rt.exitStatus = function () { return exitCode; };

    function formatUncaught(e) {
        if (e && e.__php === 2) {
            const msg = e.props["message"] || "";
            const st = e.__stack ? ("\n" + e.__stack) : "";
            return "Fatal error: Uncaught " + e.cls.name + (msg ? ": " + msg : "") + st;
        }
        return "Fatal error: " + String(e);
    }

    function finalizeRequest() {
        rt.runShutdown();
        rt.runDestructors();
        rt.persistSession();
        rt.flushAllOb();
    }

    rt.runEntry = function (body) {
        // Check if host provided an HTTP SAPI request payload
        let req = null;
        if (typeof __zipp_php_sapi === "function") {
            const reqJson = __zipp_php_sapi("request", "");
            if (typeof reqJson === "string" && reqJson.length > 0) {
                try { req = JSON.parse(reqJson); } catch (_) {}
            }
        }
        rt.resetSapiState();
        rt.initSuperglobals(req);
        try {
            const holder = { code: body, statics: null };
            holder.code([], null, null, rt.globalScope);
            finalizeRequest();
            return null;
        } catch (e) {
            if (e && e.isPhpExit) {
                exitCode = e.code;
                finalizeRequest();
                return null;
            }
            const eh = rt.getExceptionHandler();
            if (eh && e && e.__php === 2) {
                try {
                    rt.call(eh, [e], null, null, false);
                    finalizeRequest();
                    return null;
                } catch (e2) {
                    if (e2 && e2.isPhpExit) { exitCode = e2.code; finalizeRequest(); return null; }
                    e = e2;
                }
            }
            finalizeRequest();
            const text = formatUncaught(e);
            exitCode = text;
            const err = new Error(text);
            err.name = "PhpFatalError";
            throw err;
        }
    };

    // Worker Mode: handle one HTTP request on an already-booted warm PHP VM!
    rt.dispatchWorkerRequest = function (reqJson) {
        const handler = rt.getWorkerHandler();
        if (!handler) {
            return JSON.stringify({
                status: 500,
                headers: [["Content-Type", "text/plain"]],
                body: "Worker script did not call zipp_handle_requests()"
            });
        }
        let req = {};
        try { req = JSON.parse(reqJson); } catch (_) {}
        rt.resetSapiState();
        rt.initSuperglobals(req);
        try {
            // Pass a convenient request array to the handler too
            const reqArr = rt.makeArr();
            rt.arrSetInPlace(reqArr, "method", (req.server && req.server.REQUEST_METHOD) || "GET");
            rt.arrSetInPlace(reqArr, "uri", (req.server && req.server.REQUEST_URI) || "/");
            rt.arrSetInPlace(reqArr, "path", (req.server && req.server.PATH_INFO) || "/");
            rt.arrSetInPlace(reqArr, "query", req.query || "");
            rt.arrSetInPlace(reqArr, "body", req.body || "");
            rt.call(handler, [reqArr], null, null, false);
            rt.persistSession();
            rt.flushAllOb();
        } catch (e) {
            if (!(e && e.isPhpExit)) {
                rt.flushAllOb();
                rt.echoStr(formatUncaught(e));
                return JSON.stringify({
                    status: 500,
                    headers: [["Content-Type", "text/plain; charset=UTF-8"]],
                    body: ""
                });
            }
            rt.persistSession();
            rt.flushAllOb();
        }
        return rt.takeSapiResponse();
    };
})(__zipp_php);
function __zipp_php_entry() {}
__zipp_php.__rt.runEntry(__zipp_php_entry);
