//! `instanceof` in hot loops, through the loop runner (`vm/loop_tier.rs`),
//! against Node: the operator's fast path (`Vm::instanceof_fast`: a function or
//! class with the intrinsic @@hasInstance, its `prototype` read without a
//! getter, no Proxy on the chain) answered in the runner, and everything else
//! left to the dispatch arm. Classes three levels deep, plain constructor
//! functions, Object, Array and Function over mixed values; a prototype replaced
//! while the loop runs and an object's prototype set to it; a class's own
//! @@hasInstance, one defined while the loop runs, and a bound function; a Proxy
//! on the left and as the constructor, a right side that is not callable and a
//! prototype that is not an object, which throw. In the interpreter with the
//! runner entering as it does by itself, from every loop's first back-edge
//! (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and without frames
//! of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with
//! every function compiled at once.

const SOURCE: &str = r##"// `instanceof` in hot loops: the loop runner answers it by the operator's fast path (an ordinary function or class
// with the intrinsic @@hasInstance, no Proxy on the chain) and leaves everything else to the dispatch loop.
var out = [];

// 1. Classes three levels deep, plain constructor functions, Object, Array and Function, over mixed values.
class A { }
class B extends A { }
class C extends B { }
function F() { }
function G() { }
G.prototype = Object.create(F.prototype);
function mixed(n) {
  var vs = [new A(), new B(), new C(), new F(), new G(), [], {}, function () { }, "s", 3, null, undefined, Object.create(null)];
  var cs = [A, B, C, F, G, Object, Array, Function];
  var t = 0, bits = 0;
  for (var i = 0; i < n; i++) {
    var v = vs[i % vs.length], c = cs[(i >> 2) % cs.length];
    if (v instanceof c) { t++; bits = (bits * 31 + (i % vs.length) * 8 + ((i >> 2) % cs.length)) | 0; }
  }
  return t + ":" + bits;
}
out.push(mixed(40000));

// 2. A prototype replaced while the loop runs, and an object whose prototype is set to it.
function retargeted(n) {
  function P() { }
  var o = new P(), t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) P.prototype = { x: 1 };
    if (i === (3 * n) / 4) Object.setPrototypeOf(o, P.prototype);
    if (o instanceof P) t++;
  }
  return t;
}
out.push(retargeted(4000));

// 3. Symbol.hasInstance of a class's own (called each time), one defined while the loop runs, and a bound function.
class Even { static [Symbol.hasInstance](x) { return typeof x === "number" && x % 2 === 0; } }
function Late() { }
function hasInstance(n) {
  var t = 0, calls = 0, bound = C.bind(null);
  for (var i = 0; i < n; i++) {
    if (i === n / 2) Object.defineProperty(Late, Symbol.hasInstance, { value: function (x) { calls++; return x === 1; } });
    t += (i instanceof Even ? 1 : 0) + (1 instanceof Late ? 10 : 0) + (new C() instanceof bound ? 100 : 0);
  }
  return t + ":" + calls;
}
out.push(hasInstance(3000));

// 4. A Proxy on the left, a Proxy as the constructor, a right side that is not callable, and a non-object prototype.
function exotic(n) {
  var p = new Proxy(new B(), {}), seen = 0, caught = "";
  var trapped = new Proxy(function () { }, { get: function (t, k) { seen++; return t[k]; } });
  var t = 0;
  for (var i = 0; i < n; i++) { t += (p instanceof A ? 1 : 0) + ({} instanceof trapped ? 10 : 0); }
  try { for (var j = 0; j < 300; j++) { t += [] instanceof (j === 299 ? {} : Array) ? 1 : 0; } } catch (e) { caught += e.constructor.name; }
  function NoProto() { }
  NoProto.prototype = 5;
  try { for (var k = 0; k < 300; k++) { t += (k === 299 ? {} : 5) instanceof NoProto ? 1 : 0; } } catch (e) { caught += "," + e.constructor.name; }
  return t + ":" + (seen > 0) + ":" + caught;
}
out.push(exotic(3000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "7307:1841005549\n3000\n316500:1500\n3299:true:TypeError,TypeError";

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
fn loop_instanceof_worker() {
    if std::env::var_os("ZIPP_LOOP_INSTANCEOF_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_instanceof_match_node() {
    if std::env::var_os("ZIPP_LOOP_INSTANCEOF_WORKER").is_some() {
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
            .args(["loop_instanceof_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_INSTANCEOF_WORKER", "1");
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
