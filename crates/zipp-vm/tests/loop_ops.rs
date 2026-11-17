//! `~x` and `Array.isArray(x)` in hot loops, through the loop runner
//! (`vm/loop_tier.rs`), against Node: a Number's ToInt32 inverted, and isArray
//! answered while `Array.isArray` is the intrinsic, in the runner; a BigInt, a
//! value whose conversion runs code, another callee and a revoked Proxy left to
//! the dispatch arm. `~` on Int32s and on doubles past the Int32 range,
//! fractions, -0, NaN and the infinities; on strings, booleans, null, undefined,
//! an object with valueOf and BigInts; Array.isArray over arrays, objects,
//! strings, typed arrays, an arguments object, Proxies of an array and of an
//! object, Array.prototype and an array subclass; Array.isArray replaced while
//! the loop runs and restored; a revoked Proxy, which throws. In the interpreter
//! with the runner entering as it does by itself, from every loop's first
//! back-edge (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and
//! without frames of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by
//! default; and with every function compiled at once.

const SOURCE: &str = r##"// `~x` and `Array.isArray(x)` in hot loops: the loop runner inverts a Number's ToInt32 and answers isArray while
// `Array.isArray` is the intrinsic; a BigInt, a value whose conversion runs code, another callee and a revoked
// Proxy are the dispatch loop's.
var out = [];

// 1. `~` on Int32s and on doubles past the Int32 range, fractions, -0, NaN and the infinities.
function nots(n) {
  var xs = [0, 1, -1, 2147483647, -2147483648, 4294967301, -2147483649, 1.5, -1.5, -0, NaN, Infinity, -Infinity, 1e21, 2 ** 53 + 2], t = 0;
  for (var i = 0; i < n; i++) { var x = xs[i % xs.length]; t = (t + ~x + ~~(x / 3) + ~(i * 7)) | 0; }
  return t;
}
out.push(nots(30000));

// 2. `~` on strings, booleans, null, undefined, an object with valueOf (called each time) and BigInts.
function others(n) {
  var calls = 0, o = { valueOf: function () { calls++; return 6; } }, ys = ["5", "x", true, null, undefined, "0x10", " 12 "], t = 0, b = 0n;
  for (var i = 0; i < n; i++) { t = (t + ~ys[i % ys.length] + ~o) | 0; if (i % 100 === 0) b += ~BigInt(i); }
  return t + ":" + calls + ":" + b;
}
out.push(others(7000));

// 3. Array.isArray over arrays, holey and empty arrays, objects, strings, typed arrays, arguments, a Proxy of an
// array and of an object, Array.prototype itself, and an array subclass.
class MyArray extends Array { }
function isArrays(n) {
  var args = (function () { return arguments; })(1, 2);
  var vs = [[1, 2], [, 1], [], { length: 0 }, "ab", new Int8Array(2), args, new Proxy([1], {}), new Proxy({}, {}), Array.prototype, new MyArray(), null, 3];
  var t = 0, mask = 0;
  for (var i = 0; i < n; i++) { var k = i % vs.length; if (Array.isArray(vs[k])) { t++; mask |= 1 << k; } }
  return t + ":" + mask.toString(2);
}
out.push(isArrays(26000));

// 4. Array.isArray replaced while the loop runs, then restored; and a revoked Proxy, which throws.
function replaced(n) {
  var real = Array.isArray, seen = 0, t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) Array.isArray = function (x) { seen++; return x === 5; };
    if (i === (3 * n) / 4) Array.isArray = real;
    t += Array.isArray(i & 1 ? [i] : 5) ? 1 : 0;
  }
  var r = Proxy.revocable([], {}), caught = "";
  r.revoke();
  for (var j = 0; j < 400; j++) { try { t += Array.isArray(j === 399 ? r.proxy : [j]) ? 1 : 0; } catch (e) { caught = e.constructor.name; } }
  return t + ":" + seen + ":" + caught;
}
out.push(replaced(4000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "1794773224\n-90000:7000:-241570\n12000:11010000111\n2399:1000:TypeError";

/// Every switch a mode below may set, cleared before each run.
const SWITCHES: [&str; 6] = [
    "ZIPP_NOJIT",
    "ZIPP_JIT_THRESHOLD",
    "ZIPP_LOOP_RUNNER_EAGER",
    "ZIPP_NO_LOOP_RUNNER",
    "ZIPP_NO_LOOP_FRAMES",
    "ZIPP_NO_LITE",
];

#[test]
fn loop_ops_worker() {
    if std::env::var_os("ZIPP_LOOP_OPS_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_ops_match_node() {
    if std::env::var_os("ZIPP_LOOP_OPS_WORKER").is_some() {
        return;
    }
    let exe = std::env::current_exe().expect("test executable");
    let modes: [(&str, &[&str]); 7] = [
        ("default", &[]),
        ("interpreter", &["ZIPP_NOJIT"]),
        ("interpreter, runner from the first back-edge", &["ZIPP_NOJIT", "ZIPP_LOOP_RUNNER_EAGER"]),
        (
            "interpreter, runner from the first back-edge, no lite loop",
            &["ZIPP_NOJIT", "ZIPP_LOOP_RUNNER_EAGER", "ZIPP_NO_LITE"],
        ),
        (
            "interpreter, runner from the first back-edge, no frame of its own",
            &["ZIPP_NOJIT", "ZIPP_LOOP_RUNNER_EAGER", "ZIPP_NO_LOOP_FRAMES"],
        ),
        ("interpreter, no runner", &["ZIPP_NOJIT", "ZIPP_NO_LOOP_RUNNER"]),
        ("compile-at-once", &["ZIPP_JIT_THRESHOLD"]),
    ];
    for (mode, vars) in modes {
        let mut command = std::process::Command::new(&exe);
        command
            .args(["loop_ops_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_OPS_WORKER", "1");
        for v in SWITCHES {
            command.env_remove(v);
        }
        for v in vars {
            command.env(v, "1");
        }
        let out = command.output().expect("spawn the worker");
        assert!(
            out.status.success(),
            "{mode} diverged from node:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
