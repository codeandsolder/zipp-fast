//! `new` in hot loops, through the loop runner (`vm/loop_tier.rs`), against Node:
//! the instance of a constructor whose body only stores made in the runner, the
//! frame of any other plain constructor function pushed there and its return
//! handing back the instance, and everything else left to the dispatch arm. A
//! store-only body and one with a loop of its own; an object returned in place of
//! the instance and a primitive that is not; `new.target` and the instance's
//! prototype; fewer and more arguments than parameters, `arguments` in the body,
//! a default parameter; the prototype and a constructor replaced while the loop
//! runs, and a constructor that throws; classes, a bound function, natives and an
//! arrow, which throws; instances made while collections run; a constructor
//! called from a constructor, and recursion through `new`; base classes with no
//! field initializer whose constructors take frames (a loop, branches, a call, an
//! object returned, `new.target`, a private method, an implicit constructor, a
//! throw). In the interpreter with the runner entering as it does by itself, from
//! every loop's first back-edge (`ZIPP_LOOP_RUNNER_EAGER`) with and without the
//! lite loop and without frames of its own (`ZIPP_NO_LOOP_FRAMES`), and switched
//! off; by default; and with every function compiled at once.

const SOURCE: &str = r##"// `new` in hot loops: the loop runner makes the instance of a constructor whose body only stores, and pushes the
// frame of any other plain constructor function, whose return hands back the instance; everything else is the
// dispatch loop's.
var out = [];

// 1. A body that only stores its arguments, and one with a loop of its own.
function Pt(a) { this.a = a; this.b = a + 1; }
function Acc(n) { var s = 0; for (var i = 0; i < n; i++) s += i; this.s = s; this.n = n; }
function make(n) { var t = 0; for (var i = 0; i < n; i++) { var p = new Pt(i); var q = new Acc(i & 7); t = (t + p.b + q.s + q.n) | 0; } return t; }
out.push(make(20000));

// 2. Results: an object returned replaces the instance, a primitive does not; `new.target` and the prototype.
function Obj(i) { this.x = i; if (i & 1) return { y: i }; return 7; }
function Target() { this.same = new.target === Target; this.kind = typeof new.target; }
Target.prototype.tag = function () { return this.same ? 1 : 0; };
function results(n) {
  var t = 0, w = 0;
  for (var i = 0; i < n; i++) {
    var o = new Obj(i);
    if (o.y !== undefined) t += 2; else if (o.x === i && o instanceof Obj) t += 1;
    var g = new Target();
    w += g.tag() + (g.kind === "function" ? 10 : 0) + (Object.getPrototypeOf(g) === Target.prototype ? 100 : 0);
  }
  return t + ":" + w;
}
out.push(results(6000));

// 3. Fewer and more arguments than parameters; `arguments` in the body; a default parameter.
function Three(a, b, c) { this.v = (a | 0) + (b | 0) * 10 + (c | 0) * 100; }
function Args() { this.n = arguments.length; this.last = arguments[arguments.length - 1]; }
function Def(a, b = 5) { this.v = a + b; }
function arities(n) {
  var t = 0;
  for (var i = 0; i < n; i++) { t += new Three(i).v + new Three(i, 1).v + new Three(i, 1, 2, 3).v + new Args(i, 2, 9).last + new Args().n + new Def(i).v; }
  return t;
}
out.push(arities(3000));

// 4. The prototype replaced while the loop runs; a constructor replaced; one that throws.
function Pr(i) { this.i = i; }
Pr.prototype.get = function () { return this.i; };
var Swap = function (i) { this.v = i; };
function Boom(i) { if (i % 1000 === 999) throw new Error("boom " + i); this.i = i; }
function changes(n) {
  var t = 0, caught = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 2) { Pr.prototype = { get: function () { return -1; } }; Swap = function (i) { this.v = -i; }; }
    t += new Pr(i).get() + new Swap(i).v;
    try { t += new Boom(i).i; } catch (e) { caught++; t -= e.message.length; }
  }
  return t + ":" + caught;
}
out.push(changes(4000));

// 5. Classes (a base class with fields, a derived class), a bound function, natives, and a non-constructor.
class Base { f = 1; constructor(i) { this.i = i; } }
class Derived extends Base { constructor(i) { super(i); this.d = i * 2; } }
var Bound = Pt.bind(null, 3);
function others(n) {
  var t = 0;
  for (var i = 0; i < n; i++) {
    t += new Base(i).f + new Derived(i).d + new Bound().b + new Array(i & 3).length + new Map([[i, 1]]).size;
  }
  try { var arrow = () => 1; for (var j = 0; j < 300; j++) { t += j === 299 ? new arrow() : 1; } } catch (e) { t += ":" + (e instanceof TypeError); }
  return t;
}
out.push(others(3000));

// 6. Instances made while a collection runs, kept in a list and dropped.
function Node2(v, next) { this.v = v; this.next = next; this.pad = [v, v]; }
function churn(n) {
  var head = null, t = 0;
  for (var i = 0; i < n; i++) {
    head = new Node2(i, (i & 1023) === 0 ? null : head);
    t = (t + head.pad[1]) | 0;
  }
  var len = 0;
  for (var p = head; p; p = p.next) len++;
  return t + ":" + len;
}
out.push(churn(300000));

// 7. A constructor called from a constructor, and recursion through `new`.
function Inner(i) { this.i = i; }
function Outer(i) { this.in = new Inner(i + 1); this.o = i; }
function Tree(d) { this.l = d > 0 ? new Tree(d - 1) : null; this.r = d > 0 ? new Tree(d - 1) : null; this.d = d; }
function count(t) { return t ? 1 + count(t.l) + count(t.r) : 0; }
function nested(n) { var t = 0; for (var i = 0; i < n; i++) { t += new Outer(i).in.i; if ((i & 255) === 0) t += count(new Tree(6)); } return t; }
out.push(nested(5000));

// 8. Base classes with no field initializer whose constructors take frames: a loop, branches, a call, an object
// returned in place of the instance, `new.target`, a private method, an implicit constructor, and a throw.
class Looper { constructor(n) { var s = 0; for (var i = 0; i < n; i++) s += i; this.s = s; } get twice() { return this.s * 2; } }
class Brancher { constructor(i) { if (i & 1) { this.odd = true; } else { this.even = i; } } }
function helper(i) { return i * 3; }
class Caller { constructor(i) { this.v = helper(i) + helper(i + 1); } }
class Returner { constructor(i) { if (i % 3 === 0) return { other: i }; this.mine = i; } }
class Targeted { constructor() { this.t = new.target === Targeted; this.k = typeof new.target; } }
class WithPrivate { #m() { return 5; } constructor(i) { this.v = this.#m() + i; } has(o) { return #m in o; } }
class Implicit { m() { return 1; } }
class Thrower { constructor(i) { if (i % 700 === 699) throw new RangeError("t" + i); this.i = i; } }
function classes(n) {
  var t = 0, caught = 0, w = new WithPrivate(0);
  for (var i = 0; i < n; i++) {
    var l = new Looper(i & 7); t += l.twice + (l.constructor === Looper ? 1 : 0);
    var b = new Brancher(i); t += b.odd ? 1 : b.even;
    t += new Caller(i).v;
    var r = new Returner(i); t += r.other !== undefined ? 100 : r.mine + (r.constructor === Returner ? 1 : 0);
    var g = new Targeted(); t += (g.t ? 1 : 0) + g.k.length;
    var p = new WithPrivate(i); t += p.v + (w.has(p) ? 1 : 0);
    t += new Implicit().m() + (new Implicit().constructor === Implicit ? 1 : 0);
    try { t += new Thrower(i).i; } catch (e) { caught++; t -= e.message.length; }
  }
  return t + ":" + caught;
}
out.push(classes(4000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "200220000\n9000:666000\n18696000\n5984969:4\n9019799:true\n2050177040:992\n12505040\n73580214:5";

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
fn loop_new_worker() {
    if std::env::var_os("ZIPP_LOOP_NEW_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_new_match_node() {
    if std::env::var_os("ZIPP_LOOP_NEW_WORKER").is_some() {
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
            .args(["loop_new_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_NEW_WORKER", "1");
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
