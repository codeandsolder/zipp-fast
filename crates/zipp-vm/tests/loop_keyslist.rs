//! `Object.keys` in hot loops, through the loop runner (`vm/loop_tier.rs`),
//! against Node: an object that is no Proxy enumerated in the runner while
//! `Object.keys` is the intrinsic, and a primitive, a Proxy and another callee
//! left to the dispatch arm. Objects of several layouts, integer-like keys
//! first, symbols and non-enumerable properties left out; arrays (holey, with
//! an extra property), a typed array, a function, a class instance, an
//! arguments object, an Error; an object changed while the loop runs, a Proxy's
//! ownKeys trap, a string, and null, which throws; `Object.keys` replaced while
//! the loop runs and restored. In the interpreter with the runner entering as it
//! does by itself, from every loop's first back-edge (`ZIPP_LOOP_RUNNER_EAGER`)
//! with and without the lite loop and without frames of its own
//! (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with every
//! function compiled at once.

const SOURCE: &str = r##"// `Object.keys` in hot loops: the loop runner enumerates an object that is no Proxy while `Object.keys` is the
// intrinsic; a primitive, a Proxy and another callee are the dispatch loop's.
var out = [];

// 1. Plain objects of several layouts, integer-like keys first in order, symbols and non-enumerables left out.
var sym = Symbol("s");
function plain(n) {
  var a = { b: 1, a: 2, 10: 3, 2: 4 }, b = Object.defineProperty({ x: 1 }, "hidden", { value: 2, enumerable: false });
  var c = { y: 1 };
  c[sym] = 5;
  var d = {};
  for (var k = 0; k < 40; k++) d["k" + k] = k;
  delete d.k3;
  var objs = [a, b, c, d], t = 0, s = "";
  for (var i = 0; i < n; i++) { var ks = Object.keys(objs[i & 3]); t += ks.length; if (i < 4) s += ks.slice(0, 5).join(",") + "|"; }
  return t + ":" + s;
}
out.push(plain(8000));

// 2. Arrays (holey and with extra properties), typed arrays, functions, class instances, arguments, an Error.
class P { constructor() { this.p = 1; this.q = 2; } m() { } }
function others(n) {
  var arr = [1, , 3];
  arr.extra = 1;
  var vs = [arr, new Uint8Array(3), function f(a) { }, new P(), (function () { return arguments; })(7, 8), new Error("e"), []];
  var t = 0, s = "";
  for (var i = 0; i < n; i++) { var ks = Object.keys(vs[i % vs.length]); t += ks.length; if (i < vs.length) s += ks.join(",") + "|"; }
  return t + ":" + s;
}
out.push(others(7000));

// 3. Objects changed while the loop runs, a Proxy (its ownKeys trap), strings and a primitive that throws.
function changing(n) {
  var o = { a: 1 }, calls = 0, t = 0, caught = "";
  var p = new Proxy({ z: 1, w: 2 }, { ownKeys: function (tg) { calls++; return Reflect.ownKeys(tg); } });
  for (var i = 0; i < n; i++) {
    if ((i & 7) === 0) o["n" + (i & 63)] = i;
    if ((i & 15) === 15) delete o.a;
    t += Object.keys(o).length + Object.keys(p).length + Object.keys("abc").length;
  }
  for (var j = 0; j < 300; j++) { try { t += Object.keys(j === 299 ? null : { j: j }).length; } catch (e) { caught = e.constructor.name; } }
  return t + ":" + calls + ":" + caught;
}
out.push(changing(4000));

// 4. Object.keys replaced while the loop runs, then restored.
function replaced(n) {
  var real = Object.keys, seen = 0, t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) Object.keys = function (o) { seen++; return ["only"]; };
    if (i === (3 * n) / 4) Object.keys = real;
    t += Object.keys({ a: 1, b: 2, c: i }).length;
  }
  return t + ":" + seen;
}
out.push(replaced(4000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "90000:2,10,b,a|x|y|k0,k1,k2,k4,k5|\n10000:0,2,extra|0,1,2||p,q|0,1|||\n52090:4000:TypeError\n10000:1000";

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
fn loop_keyslist_worker() {
    if std::env::var_os("ZIPP_LOOP_KEYSLIST_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_keyslist_match_node() {
    if std::env::var_os("ZIPP_LOOP_KEYSLIST_WORKER").is_some() {
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
            .args(["loop_keyslist_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_KEYSLIST_WORKER", "1");
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
