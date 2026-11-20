//! Singleton `[[IsHTMLDDA]]` membership mirror.
//!
//! The production VM creates exactly one `$262.IsHTMLDDA` object. These tests
//! pin its observable Annex-B semantics, exercise ordinary heap values as
//! negative controls, and prove both the scalar route and its HashSet fallback
//! in fresh child processes (the switches/counters latch process-wide).

use std::process::{Command, Output};

const CHILD_ENV: &str = "ZIPP_HTMLDDA_SCALAR_CHILD";
const PROBE_ENV: &str = "ZIPP_HTMLDDA_SCALAR_PROBE";
const EXPECTED: &str = "htmldda 20480 false false true true true true true";

const SOURCE: &str = r#"
  "use strict";
  (function () {
    const dda = $262.IsHTMLDDA;
    const ordinary = {};
    const text = "x";
    const callable = function () {};
    let score = 0;
    for (let i = 0; i < 2048; i++) {
      if (typeof dda === "undefined") score++;
      if (dda == null) score++;
      if (null == dda) score++;
      if (dda == undefined) score++;
      if (undefined == dda) score++;
      if (!dda) score++;
      if (typeof ordinary === "object") score++;
      if (ordinary != null) score++;
      if (text != null) score++;
      if (callable != null) score++;
    }

    // The singleton lives below the GC floor. Force a collection before the
    // final identity/Annex-B checks so a stale or unrooted mirror cannot pass.
    $262.gc();
    console.log(
      "htmldda",
      score,
      dda === undefined,
      Object.is(dda, undefined),
      dda() === null,
      dda("") === null,
      dda("x") === undefined,
      typeof dda === "undefined",
      dda == null
    );
  })();
"#;

fn execute_source() {
    let out = zipp_vm::run(SOURCE).expect("HTMLDDA source compiles");
    assert!(out.error.is_none(), "runtime error: {:?}", out.error);
    assert_eq!(out.output, [EXPECTED]);
}

fn run_child(env: &[(&str, &str)]) -> Output {
    let exe = std::env::current_exe().expect("test executable path");
    let mut cmd = Command::new(exe);
    cmd.args(["htmldda_scalar_child", "--exact", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env_remove(PROBE_ENV)
        .env_remove("ZIPP_NO_HTMLDDA_SCALAR")
        .env_remove("ZIPP_ICSTATS")
        .env_remove("ZIPP_NOJIT")
        .env_remove("ZIPP_JIT_THRESHOLD")
        .env_remove("ZIPP_GC_STRESS");
    cmd.envs(env.iter().copied());
    cmd.output().expect("HTMLDDA child runs")
}

fn assert_child(label: &str, out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && !stdout.contains("running 0 tests"),
        "{label} child failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    stdout.into_owned()
}

#[test]
fn htmldda_scalar_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    execute_source();
    if std::env::var_os(PROBE_ENV).is_some() {
        let (scalar, set) = zipp_vm::htmldda_membership_stats();
        println!("HTMLDDA_MEMBERSHIP scalar={scalar} set={set}");
    }
}

#[test]
fn htmldda_semantics_match_with_scalar_fallback_nojit_and_gc_stress() {
    execute_source();
    for (label, env) in [
        ("scalar-jit", vec![("ZIPP_JIT_THRESHOLD", "1")]),
        (
            "set-fallback",
            vec![("ZIPP_NO_HTMLDDA_SCALAR", "1"), ("ZIPP_JIT_THRESHOLD", "1")],
        ),
        ("no-jit", vec![("ZIPP_NOJIT", "1")]),
        (
            "scalar-gc-stress",
            vec![("ZIPP_JIT_THRESHOLD", "1"), ("ZIPP_GC_STRESS", "1")],
        ),
        (
            "fallback-gc-stress",
            vec![("ZIPP_NO_HTMLDDA_SCALAR", "1"), ("ZIPP_GC_STRESS", "1")],
        ),
    ] {
        assert_child(label, &run_child(&env));
    }
}

fn probe(disabled: bool, nojit: bool) -> (u64, u64) {
    let mut env = vec![(PROBE_ENV, "1"), ("ZIPP_ICSTATS", "1")];
    if disabled {
        env.push(("ZIPP_NO_HTMLDDA_SCALAR", "1"));
    }
    if nojit {
        env.push(("ZIPP_NOJIT", "1"));
    }
    let stdout = assert_child(
        if disabled {
            "fallback probe"
        } else {
            "scalar probe"
        },
        &run_child(&env),
    );
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("HTMLDDA_MEMBERSHIP "))
        .and_then(|line| {
            let mut fields = line.split_whitespace();
            let scalar = fields.next()?.strip_prefix("scalar=")?.parse().ok()?;
            let set = fields.next()?.strip_prefix("set=")?.parse().ok()?;
            Some((scalar, set))
        })
        .unwrap_or_else(|| panic!("missing HTMLDDA mechanism counters in:\n{stdout}"))
}

#[test]
fn mechanism_counter_and_kill_switch_select_exactly_one_membership_route() {
    // Under JIT, Tier C/O inline scalar HTMLDDA compares bypass `is_htmldda_index`
    // directly against `JIT_HTMLDDA_IDX_OFFSET` while remaining checks hit the
    // scalar mirror and zero probe the HashSet; the kill switch disables both
    // inline and helper scalar checks and routes everything through the HashSet.
    let (scalar_on, set_on) = probe(false, false);
    let (scalar_off, set_off) = probe(true, false);
    assert!(
        scalar_on > 10_000,
        "scalar mirror did not serve the hot membership checks: {scalar_on}"
    );
    assert_eq!(set_on, 0, "enabled mode unexpectedly probed the HashSet");
    assert_eq!(
        scalar_off, 0,
        "kill switch unexpectedly used the scalar mirror"
    );
    assert!(
        set_off > 20_000,
        "kill switch did not restore HashSet membership checks: {set_off}"
    );

    // Without JIT, every check routes through `is_htmldda_index`.
    let (interp_scalar_on, interp_set_on) = probe(false, true);
    let (interp_scalar_off, interp_set_off) = probe(true, true);
    assert!(
        interp_scalar_on > 20_000,
        "interpreter scalar mirror did not serve all checks: {interp_scalar_on}"
    );
    assert_eq!(interp_set_on, 0, "interpreter enabled mode probed HashSet");
    assert_eq!(interp_scalar_off, 0, "interpreter kill switch used scalar mirror");
    assert!(
        interp_set_off > 20_000,
        "interpreter kill switch did not restore HashSet checks: {interp_set_off}"
    );
}
