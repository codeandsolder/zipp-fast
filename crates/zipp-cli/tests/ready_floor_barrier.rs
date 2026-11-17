//! The first object literals a program builds, holding the closures and
//! strings it has just made, while minor collections run: `zipp js` under the
//! nursery verifier (`ZIPP_NURSERY_VERIFY`, which runs the full mark beside
//! every minor and panics on a live young object the minor missed) and without
//! it, in the interpreter and by default.
//!
//! The CLI's setup (its Node-compatible runtime) builds object literals of
//! its own, which can leave ready objects (a slab class's empty young objects,
//! made a batch at a time) in their buffer when `set_gc_floor` pins the boot
//! allocations. `young_reset` promoted them with the rest, and a literal that
//! then took one was an OLD object whose first stores, its young values, had
//! no barrier. No minor traced them, and a closure such a literal held was
//! swept while it was still reachable and its slot reused: a Proxy handler's
//! `ownKeys` trap became another object ("trap is not a function"), in the
//! interpreter of every release since ready objects. `young_reset` now
//! abandons the ready buffers first.
use std::process::Command;

const SOURCE: &str = r##"var calls = 0;
var h1 = { f: function () { calls++; return calls; } };
var h2 = { g: function (x) { return x + calls; }, s: "s" + calls };
var h3 = { k: [calls], m: function () { return 3; } };
var p = new Proxy({ z: 1, w: 2 }, { ownKeys: function (tg) { calls++; return Reflect.ownKeys(tg); } });
var keep = [];
for (var i = 0; i < 300000; i++) { var o = { i: i, w: [i] }; if ((i & 4095) === 0) keep.push(o); }
var t = 0;
for (var j = 0; j < 1000; j++) t += h1.f() + h2.g(j) + h3.m() + h3.k.length + h2.s.length + Object.keys(p).length;
console.log(t + " " + keep.length + " " + typeof h1.f + " " + typeof h2.g + " " + typeof h3.m + " " + calls);
"##;

/// What Node prints.
const EXPECTED: &str = "2507500 74 function function function 2000";

#[test]
fn ready_floor_literals_keep_their_values() {
    let dir = std::env::temp_dir().join(format!("zipp-ready-floor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("ready_floor.js");
    std::fs::write(&file, SOURCE).expect("write the script");
    let modes: [(&str, &[&str]); 4] = [
        ("interpreter, verified", &["ZIPP_NOJIT", "ZIPP_NURSERY_VERIFY"]),
        ("interpreter", &["ZIPP_NOJIT"]),
        ("default, verified", &["ZIPP_NURSERY_VERIFY"]),
        ("default", &[]),
    ];
    for (mode, vars) in modes {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zipp"));
        cmd.arg("js").arg(&file).env("ZIPP_CACHE_DIR", "");
        for v in ["ZIPP_NOJIT", "ZIPP_NURSERY_VERIFY"] {
            cmd.env_remove(v);
        }
        for v in vars {
            cmd.env(v, "1");
        }
        let out = cmd.output().expect("run zipp");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.trim_end() == EXPECTED,
            "{mode}:\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
