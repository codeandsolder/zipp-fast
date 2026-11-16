//! `super` property reads and writes in the functions a hot loop calls, through
//! the loop runner (`vm/loop_tier.rs`), against Node: `SuperBase`, `SuperGet`
//! and `SuperSet` by the site's super cache, a getter or setter called in a
//! frame the runner pushes or as a leaf, and everything else left to the
//! dispatch arm. A parent's method called through `super` over four layouts;
//! getters and setters through `super`, the parent's a leaf and the child's a
//! frame; a getter two prototypes up, one with a loop and one returning a
//! string; static methods and accessors; the parent's method replaced while the
//! loop runs and the child's prototype re-targeted; a write with no setter on
//! the chain, which defines the receiver's own property; a getter that throws
//! from a frame the runner pushed, and a property that is not there; a class
//! evaluated again on every call; a sloppy getter with a primitive `this`; an
//! object literal's methods. In the interpreter with the runner entering as it
//! does by itself, from every loop's first back-edge
//! (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and without frames
//! of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with
//! every function compiled at once.

const SOURCE: &str = r##"// `super` property reads and writes in the functions a hot loop calls: the loop runner reads them through the
// site's super cache, calls a getter or a setter in a frame of its own, and leaves for anything else.
"use strict";
var out = [];

// 1. A method's call of the parent's method through `super`, in a loop over four layouts.
class Shape {
  constructor(v) { this._v = v | 0; }
  area() { return this._v + 1; }
  get v() { return this._v; }
  set v(x) { this._v = x | 0; }
}
class Circle extends Shape { area() { return super.area() * 3 + 1; } }
class Square extends Shape { constructor(v) { super(v); this.side = v * 2; } area() { return super.area() * 4 + this.side; } }
class Tri extends Shape {
  area() { return super.area() * 5 + 3; }
  get v() { return super.v * 2; }
  set v(x) { super.v = x; }
}
class Hex extends Shape {
  area() { return (super.area() * 6 + 5) | 0; }
  get v() { return super.v; }
  set v(x) { super.v = x + 1; }
}
var shapes = [new Circle(11), new Square(22), new Tri(33), new Hex(44)];
function areas(n) { var t = 0; for (var i = 0; i < n; i++) { t = (t + shapes[i & 3].area()) | 0; } return t; }
out.push(areas(20000));

// 2. A getter and a setter through `super`, the parent's a leaf, the child's a frame.
function accessors(n) {
  var t = 0;
  for (var i = 0; i < n; i++) { var s = shapes[i & 3]; s.v = (i & 1023) - 7; t = (t + s.v) | 0; }
  return t + ":" + shapes.map(function (s) { return s.v; }).join(",");
}
out.push(accessors(20000));

// 3. A getter two prototypes up, one that is not a leaf, and one that returns a string.
class G0 { get deep() { var s = 0; for (var k = 0; k < 3; k++) s += k; return s + this.base; } get name() { return "g" + this.base; } }
class G1 extends G0 { }
class G2 extends G1 { constructor() { super(); this.base = 4; } read() { return super.deep + super.name.length; } }
function deep(n) { var g = new G2(), t = 0; for (var i = 0; i < n; i++) { t += g.read(); } return t; }
out.push(deep(5000));

// 4. Static methods and accessors through `super`.
class S0 { static make(i) { return i * 2; } static get tag() { return 7; } static set tag(x) { S0.last = x; } }
class S1 extends S0 { static make(i) { return super.make(i) + 1; } static get tag() { return super.tag + 1; } static set tag(x) { super.tag = x * 3; } }
function statics(n) { var t = 0; for (var i = 0; i < n; i++) { t += S1.make(i) + S1.tag; S1.tag = i; } return t + ":" + S0.last; }
out.push(statics(5000));

// 5. The parent's method replaced while the loop runs, then the child's prototype re-targeted.
class P0 { m() { return 1; } }
class P1 { m() { return 100; } }
class C0 extends P0 { m() { return super.m() + 10; } }
function retarget(n) {
  var c = new C0(), t = 0;
  for (var i = 0; i < n; i++) {
    if (i === n / 4) P0.prototype.m = function () { return 2; };
    if (i === n / 2) Object.setPrototypeOf(C0.prototype, P1.prototype);
    t += c.m();
  }
  return t;
}
out.push(retarget(4000));

// 6. A `super` write with no setter on the chain defines the receiver's own property.
class D0 { }
class D1 extends D0 { put(i) { super.slot = i; return this.slot; } }
function dataWrites(n) { var d = new D1(), t = 0; for (var i = 0; i < n; i++) { t += d.put(i & 15); } return t + ":" + Object.keys(d).join(","); }
out.push(dataWrites(3000));

// 7. A getter through `super` that throws, from a frame the runner pushed; and a property that is not there.
class T0 { get bad() { if (this.n % 500 === 499) throw new RangeError("at " + this.n); return this.n; } }
class T1 extends T0 { constructor() { super(); this.n = 0; } step() { this.n++; return super.bad + (super.missing === undefined ? 1 : 0); } }
function throwing(n) {
  var t = new T1(), s = 0, caught = 0;
  for (var i = 0; i < n; i++) { try { s += t.step(); } catch (e) { caught++; s -= e.message.length; } }
  return s + ":" + caught;
}
out.push(throwing(3000));

// 8. A class evaluated again on every call: each evaluation is a new home.
function fresh(k) {
  class B { m() { return k; } get g() { return k * 2; } }
  class D extends B { m() { return super.m() + super.g; } }
  return new D();
}
function evaluations(n) { var t = 0; for (var j = 0; j < 20; j++) { var d = fresh(j); for (var i = 0; i < n; i++) { t += d.m(); } } return t; }
out.push(evaluations(300));

// 9. A sloppy getter on the parent read through `super` with a primitive `this`, and an object's.
class Q0 { }
Object.defineProperty(Q0.prototype, "kind", { get: Function("return typeof this"), configurable: true });
class Q1 extends Q0 { kind2() { return super.kind; } }
function primitives(n) {
  var q = new Q1(), t = "";
  for (var i = 0; i < n; i++) { var r = Q1.prototype.kind2.call(i & 1 ? 5 : q); if (i < 4) t += r + ","; }
  return t;
}
out.push(primitives(2000));

// 10. Object literal methods with `super`.
var base = { hi() { return "b" + this.k; }, get k2() { return this.k * 2; } };
var obj = { __proto__: base, k: 3, hi() { return super.hi() + super.k2; } };
function literals(n) { var t = 0; for (var i = 0; i < n; i++) { t += obj.hi().length; } return t; }
out.push(literals(4000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "3105000\n12456800:533,534,1070,537\n45000\n25040000:14997\n243000\n22468:slot\n4493960:6\n171000\nobject,object,object,object,\n12000";

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
fn loop_super_worker() {
    if std::env::var_os("ZIPP_LOOP_SUPER_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_super_match_node() {
    if std::env::var_os("ZIPP_LOOP_SUPER_WORKER").is_some() {
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
            .args(["loop_super_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_SUPER_WORKER", "1");
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
