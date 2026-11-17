//! The `in` operator in hot loops, through the loop runner (`vm/loop_tier.rs`),
//! against Node: an array's index answered in the runner, present, a hole or
//! past the end, and everything else left to the dispatch arm. A holey array
//! walked hole-aware in a function and at the top level; keys that are not
//! small integers (doubles, negatives, strings, 2^32 - 1, NaN); a hole an index
//! on `Array.prototype` fills, set while the loop runs; an array whose
//! prototype is replaced while the loop runs, and one given an accessor index;
//! an arguments object, a plain object, a typed array, a string object, a Proxy
//! with a `has` trap, and a primitive right operand, which throws; an array
//! that grows and shrinks while the loop runs. In the interpreter with the
//! runner entering as it does by itself, from every loop's first back-edge
//! (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and without frames
//! of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with
//! every function compiled at once.

const SOURCE: &str = r##"// The `in` operator in hot loops: the loop runner answers an array's index itself, present or absent, and leaves
// everything else (another kind of key or object, an index something else can supply) to the dispatch loop.
var out = [];

// 1. A holey array walked hole-aware, and indexes past its end.
var holey = [];
for (var i = 0; i < 5000; i++) holey[i] = i * 3;
for (var i = 0; i < 5000; i += 7) delete holey[i];
function walk(a, n) { var s = 0, h = 0; for (var i = 0; i < n; i++) { if (i in a) s = (s + a[i]) | 0; else h++; } return s + ":" + h; }
out.push(walk(holey, 6000));

// 2. A top-level loop over the same array, with globals.
var gs = 0, gh = 0;
for (var gi = 0; gi < 6000; gi++) { if (gi in holey) gs = (gs + holey[gi]) | 0; else gh++; }
out.push(gs + ":" + gh);

// 3. Keys that are not small integers: doubles, negatives, strings, 2^32 - 1, NaN.
function keys(a, n) {
  var t = 0;
  for (var i = 0; i < n; i++) {
    if ((i / 2) in a) t++;
    if (-i in a) t += 10;
    if (String(i) in a) t += 100;
    if (4294967295 in a) t += 1000;
    if (NaN in a) t += 10000;
  }
  return t;
}
var withKeys = [1, 2, 3, 4, 5, 6, 7, 8];
withKeys[4294967295] = 1;
withKeys["-3"] = 1;
withKeys.NaN = 1;
out.push(keys(withKeys, 3000));

// 4. A hole an index on Array.prototype fills, set while the loop runs, then removed.
function protoIndex(n) {
  var a = [1, , 3, , 5], t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) Array.prototype[3] = "p";
    if (3 in a) t++;
    if (1 in a) t += 2;
  }
  delete Array.prototype[3];
  return t + ":" + (3 in a);
}
out.push(protoIndex(4000));

// 5. An array whose prototype is replaced while the loop runs, and one whose index is an accessor.
function otherProto(n) {
  var a = [1, , 3], t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) Object.setPrototypeOf(a, { 1: "q" });
    if (1 in a) t++;
  }
  return t;
}
out.push(otherProto(4000));
function accessorIndex(n) {
  var a = [1, 2, 3], t = 0, reads = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) { delete a[1]; Object.defineProperty(a, 5, { get: function () { reads++; return 1; }, configurable: true }); }
    if (1 in a) t++;
    if (5 in a) t += 10;
  }
  return t + ":" + reads + ":" + a.length;
}
out.push(accessorIndex(4000));

// 6. An arguments object, a plain object, a typed array, a string object, a Proxy, and a primitive that throws.
function argsIn() { var t = 0; for (var i = 0; i < 3000; i++) { if ((i & 3) in arguments) t++; } return t; }
out.push(argsIn(7, 8, 9));
function objects(n) {
  var o = { 0: 1, 2: 3 }, ta = new Int8Array(4), so = new String("abc"), seen = 0, t = 0;
  var p = new Proxy([1, 2], { has: function (target, k) { seen++; return k === "1"; } });
  for (var i = 0; i < n; i++) { var k = i & 3; if (k in o) t++; if (k in ta) t += 10; if (k in so) t += 100; if (k in p) t += 1000; }
  try { for (var j = 0; j < 300; j++) { t += (j in (j === 299 ? 5 : holey)) ? 1 : 0; } } catch (e) { t += ":" + (e instanceof TypeError); }
  return t + ":" + seen;
}
out.push(objects(4000));

// 7. An array that grows and shrinks while the loop runs.
function growing(n) {
  var a = [], t = 0;
  for (var i = 0; i < n; i++) {
    if ((i & 1) === 0) a.push(i); else if (a.length > 50) a.length = 10;
    if ((i % 37) in a) t++;
    if (60 in a) t += 1000;
  }
  return t;
}
out.push(growing(5000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "32132145:1715\n32132145:1715\n33000828\n2000:false\n2000\n22000:0:6\n2250\n1342256:true:4000\n3781";

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
fn loop_in_worker() {
    if std::env::var_os("ZIPP_LOOP_IN_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_in_match_node() {
    if std::env::var_os("ZIPP_LOOP_IN_WORKER").is_some() {
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
            .args(["loop_in_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_IN_WORKER", "1");
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
