//! Compound assignments to computed members in hot loops (`o[k] += v`,
//! `o[k]++`), through the loop runner (`vm/loop_tier.rs`), against Node: the
//! key taken as it is when it is no object (`ToPropKey`), and an object key,
//! whose conversion runs code, and a null or undefined base, which throws, left
//! to the dispatch arm. Arrays and typed arrays with integer keys; string and
//! symbol keys, numeric strings, -0, 1.5, NaN, undefined, null and true as keys;
//! object keys converted by their `toString` (Node converts such a key twice
//! per compound assignment, the specification once, so only the result is
//! observed); a base that becomes null or undefined, which throws before the
//! key is converted; a string and a number as bases. In the interpreter with
//! the runner entering as it does by itself, from every loop's first back-edge
//! (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and without frames
//! of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with
//! every function compiled at once.

const SOURCE: &str = r##"// Compound assignments to computed members in hot loops (`o[k] += v`, `o[k]++`): the loop runner takes the key as
// it is when it is no object, and leaves an object key, whose conversion runs code, and a null or undefined base,
// which throws, to the dispatch loop.
var out = [];

// 1. Arrays and typed arrays, integer keys.
function counts(n) {
  var a = [0, 0, 0, 0, 0, 0, 0, 0], f = new Float64Array(8), u = new Uint8Array(8), t = 0;
  for (var i = 0; i < n; i++) { a[i & 7] += i; f[(i * 3) & 7] += 0.5; u[i & 7]++; a[(i + 1) & 7] -= 1; }
  for (var j = 0; j < 8; j++) t += a[j] + f[j] + u[j];
  return t;
}
out.push(counts(40000));

// 2. String and symbol keys, numeric strings, and keys that are not small integers.
var sym = Symbol("s");
function keyed(n) {
  var o = { a: 0, b: 0 }, h = [1, 2, 3], keys = ["a", "b", "1", "2"], t = "";
  o[sym] = 0;
  for (var i = 0; i < n; i++) {
    o[keys[i & 1]] += 1;
    h[keys[2 + (i & 1)]] += 2;
    o[sym] += 3;
    o[-0] = (o[-0] | 0) + 1;
    o[1.5] = (o[1.5] | 0) + 1;
    o[NaN] = (o[NaN] | 0) + 1;
    o[undefined] = (o[undefined] | 0) + 1;
    o[null] = (o[null] | 0) + 1;
    o[true] = (o[true] | 0) + 1;
  }
  for (var k of Object.keys(o).sort()) t += k + "=" + o[k] + ",";
  return t + h.join("|") + "|" + o[sym];
}
out.push(keyed(5000));

// 3. An object key, converted by its toString (Node calls it twice per compound assignment, the specification
// once: only its result is observed here).
function objectKeys(n) {
  var o = { k0: 0, k1: 0 }, keys = [{ toString: function () { return "k0"; } }, { toString: function () { return "k1"; } }];
  for (var i = 0; i < n; i++) { o[keys[i & 1]] += i; }
  return o.k0 + ":" + o.k1;
}
out.push(objectKeys(3000));

// 4. A base that becomes null or undefined: a TypeError before the key's conversion.
function nullish(n) {
  var o = { x: 0 }, seen = 0, caught = "";
  var key = { toString: function () { seen++; return "x"; } };
  for (var i = 0; i < n; i++) {
    var b = i === n - 2 ? null : i === n - 1 ? undefined : o;
    var before = seen;
    try { b[key] += 1; } catch (e) { caught += e.constructor.name + (seen - before) + ","; }
  }
  return o.x + ":" + caught;
}
out.push(nullish(2000));

// 5. Primitive bases: a string's index, a number's property (sloppy writes to primitives are ignored).
function primitives(n) {
  var s = "hello", t = 0, num = 5;
  for (var i = 0; i < n; i++) { var c = i % 5; t += s[c].charCodeAt(0); num[c] += 1; }
  return t + ":" + num;
}
out.push(primitives(3000));

console.log(out.join("\n"));
"##;

const EXPECTED: &str = "799961088\n0=5000,1.5=5000,NaN=5000,a=2500,b=2500,null=5000,true=5000,undefined=5000,1|5002|5003|15000\n2248500:2250000\n1998:TypeError0,TypeError0,\n319200:5";

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
fn loop_keys_worker() {
    if std::env::var_os("ZIPP_LOOP_KEYS_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_keys_match_node() {
    if std::env::var_os("ZIPP_LOOP_KEYS_WORKER").is_some() {
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
            .args(["loop_keys_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_KEYS_WORKER", "1");
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
