//! Object literals built in hot loops, through the loop runner
//! (`vm/loop_tier.rs`), against Node: `FinalizeObject` with its key plan
//! checked once when the function's positions are built and its values taken
//! straight from the register window. Literals of three keys nested in
//! another; two key orders in one loop; sixteen keys, the most a plan stages;
//! literals kept across collections while others die. In the interpreter with
//! the runner entering as it does by itself, from every loop's first back-edge
//! (`ZIPP_LOOP_RUNNER_EAGER`) with and without the lite loop and without frames
//! of its own (`ZIPP_NO_LOOP_FRAMES`), and switched off; by default; and with
//! every function compiled at once.

const SOURCE: &str = r##"// Object literals in hot loops: the loop runner builds them from the site's key plan, checked once, and the window's values.
var out = [];
function lits(n) { var t = 0; for (var i = 0; i < n; i++) { var o = { a: i, b: 2, c: "x" + (i & 3) }; var p = { q: o, r: [i] }; t = (t + o.a + o.b + o.c.length + p.q.a + p.r[0]) | 0; } return t; }
out.push(lits(200000));
function shapes(n) { var t = 0; for (var i = 0; i < n; i++) { var o = (i & 1) ? { x: i, y: 1 } : { y: 1, x: i }; t += Object.keys(o).join("").length + o.x; } return t; }
out.push(shapes(50000));
function many(n) { var t = 0; for (var i = 0; i < n; i++) { var o = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9, j: 10, k: 11, l: 12, m: 13, n: 14, o: 15, p: i }; t = (t + o.p + o.a + o.o) | 0; } return t; }
out.push(many(100000));
var keep = []; for (var i = 0; i < 300000; i++) { var o = { v: i, w: { z: i } }; if ((i % 1000) === 0) keep.push(o); }
out.push(keep.length + ":" + keep[17].w.z);
console.log(out.join("\n"));
"##;

const EXPECTED: &str = "-129042144\n1250075000\n706582704\n300:17000";

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
fn loop_literals_worker() {
    if std::env::var_os("ZIPP_LOOP_LITERALS_WORKER").is_none() {
        return;
    }
    let out = zipp_vm::run(SOURCE).expect("source compiles");
    assert!(out.error.is_none(), "unexpected runtime error: {:?}", out.error);
    assert_eq!(out.output.join("\n"), EXPECTED);
}

#[test]
fn loop_literals_match_node() {
    if std::env::var_os("ZIPP_LOOP_LITERALS_WORKER").is_some() {
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
            .args(["loop_literals_worker", "--exact", "--nocapture"])
            .env("ZIPP_LOOP_LITERALS_WORKER", "1");
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
