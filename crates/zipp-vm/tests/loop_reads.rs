//! Property reads no cache answers, in hot loops, through the loop runner
//! (`vm/loop_tier.rs`), against Node: finished in the runner by the dispatch
//! arm's last steps (a constructor's own data, then the general read, which may
//! run a getter or a Proxy's trap) instead of leaving it. A class instance's and
//! a function instance's `constructor` and its name; a Proxy's get trap, a
//! null-prototype object and absent properties; a function's, a class's, a
//! Map's, a RegExp's, an arguments object's and an Error's properties; a getter
//! defined and redefined while the loop runs, and one that throws; a number's
//! and a boolean's properties, and an undefined base, which throws. In the
//! interpreter with the runner entering as it does by itself, from every loop's
//! first back-edge (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and
//! without frames of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by
//! default; and with every function compiled at once.

const SOURCE: &str = r##"// Property reads no cache answers, in hot loops: the loop runner finishes them as the dispatch loop does (a
// constructor's own data, then the general read, which may run a getter or a Proxy's trap) instead of leaving.
var out = [];

// 1. A class instance's and a function instance's `constructor`, and its name.
class K { constructor(i) { this.i = i; } }
function Q(i) { this.i = i; }
function ctors(n) {
  var a = [new K(1), new Q(2), [], {}, "s", 5], t = 0, names = "";
  for (var i = 0; i < n; i++) {
    var o = a[i % a.length];
    t += (o.constructor === K ? 1 : 0) + (o.constructor === Q ? 2 : 0) + (o.constructor === Array ? 4 : 0);
    if (i < a.length) names += o.constructor.name + ",";
  }
  return t + ":" + names;
}
out.push(ctors(30000));

// 2. A Proxy's get trap (called each time), a null-prototype object, and properties that are not there.
function exotic(n) {
  var calls = 0, p = new Proxy({ x: 1 }, { get: function (t, k) { calls++; return k === "x" ? 7 : undefined; } });
  var bare = Object.create(null);
  bare.y = 3;
  var t = 0;
  for (var i = 0; i < n; i++) { t += p.x + bare.y + (bare.z === undefined ? 1 : 0) + ({}.missing === undefined ? 1 : 0); }
  return t + ":" + calls;
}
out.push(exotic(5000));

// 3. Functions, classes, a Map, a RegExp, an arguments object and an Error.
class S { static tag = 9; }
function reads(n) {
  var f = function named(a, b) { }, m = new Map([[1, 2]]), re = /ab+c/gi, e = new RangeError("r"), t = 0, s = "";
  var args = (function () { return arguments; })(1, 2, 3);
  for (var i = 0; i < n; i++) {
    t += f.length + S.tag + m.size + re.lastIndex + args.length + e.message.length + (re.global ? 1 : 0);
    if (i === 0) s = f.name + re.source + re.flags + e.name;
  }
  return t + ":" + s;
}
out.push(reads(5000));

// 4. A getter on a prototype defined and redefined while the loop runs, and one that throws.
function getters(n) {
  function P() { }
  var o = new P(), t = 0, caught = 0, kinds = "";
  for (var i = 0; i < n; i++) {
    if (i === n / 4) Object.defineProperty(P.prototype, "g", { get: function () { return 2; }, configurable: true });
    if (i === n / 2) Object.defineProperty(P.prototype, "g", { get: function () { if (i % 100 === 99) throw new RangeError("g"); return 3; }, configurable: true });
    try { t += o.g === undefined ? 1 : o.g; } catch (err) { caught++; if (kinds.length < 20) kinds += err.constructor.name[0]; }
  }
  return t + ":" + caught + ":" + kinds;
}
out.push(getters(4000));

// 5. Primitive receivers (a number's and a boolean's properties) and an undefined base, which throws.
function primitives(n) {
  var vals = [5, 2.5, true, false], t = 0, caught = "";
  for (var i = 0; i < n; i++) {
    var v = vals[i & 3];
    t += (v.constructor === Number ? 1 : 0) + (v.constructor === Boolean ? 2 : 0) + (typeof v.toString === "function" ? 4 : 0) + (v.nope === undefined ? 8 : 0);
  }
  for (var j = 0; j < 300; j++) { var b = j === 299 ? undefined : { x: j }; try { t += b.x; } catch (e) { caught = e.constructor.name; } }
  return t + ":" + caught;
}
out.push(primitives(8000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "35000:K,Q,Array,Object,String,Number,\n60000:5000\n85000:namedab+cgiRangeError\n8940:20:RRRRRRRRRRRRRRRRRRRR\n152551:TypeError";

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
fn loop_reads_worker() {
    if std::env::var_os("ZIPP_LOOP_READS_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_reads_match_node() {
    if std::env::var_os("ZIPP_LOOP_READS_WORKER").is_some() {
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
            .args(["loop_reads_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_READS_WORKER", "1");
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
