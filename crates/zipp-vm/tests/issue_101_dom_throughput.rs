//! Regression and soundness tests for the #101 DOM/HTML throughput optimizations:
//! - Float32Array DOUBLE register-allocated loads/stores and NaN canonicalization
//! - `pinned_nullish_cmp` (`bias === void 0 ? 0 : bias[row]` and `!== null`)
//! - Cross-call route epoch fast path when `global_route_epoch != 0` and live invalidation
//! - Arrow lexical destructuring unboxing (and TDZ / capture preservation)
//! - Class member lookup cache and class-getter route invalidation
//! - `%RegExp.prototype%` fast-slot cache and non-global `str.match(re)` invalidation
//! - Pooled string/array/closure fast paths (`fromCodePoint`, `fromCharCode`,
//!   `slice`, `substring`, `trim*`, `toLowerCase`/`toUpperCase`, `split`, `map`,
//!   `filter`, `array.slice`, `upval_pool`)

use std::process::Command;

use zipp_vm::{compile_to_text, run};

const PROBE_PROGRAM: &str = r####""use strict";
const out = [];

// 1. Float32Array DOUBLE regalloc + NaN canonicalization (no forged NaN-box tags)
{
  const buf = new ArrayBuffer(32);
  const f32 = new Float32Array(buf);
  const u32 = new Uint32Array(buf);
  // Raw signaling/quiet/negative NaN bit patterns whose float64 promotion could
  // collide with NaN-box payload tags if not canonicalized.
  u32[0] = 0x7fc90000;
  u32[1] = 0x7ff80001;
  u32[2] = 0xffc00001;
  u32[3] = 0x7f800001;
  f32[4] = -0;
  f32[5] = Infinity;
  f32[6] = -Infinity;
  f32[7] = 1.5;

  function readElem(a, idx) {
    let v = 0;
    for (let i = 0; i < 64; i++) {
      v = a[idx];
    }
    return v;
  }
  // Warm up with normal numbers first so the loop compiles on DOUBLE tier.
  const normal = new Float32Array(8);
  for (let i = 0; i < 8; i++) normal[i] = i + 0.25;
  let warmSum = 0;
  for (let r = 0; r < 400; r++) warmSum += readElem(normal, r & 7);

  const nanChecks = [];
  for (let i = 0; i < 4; i++) {
    const v = readElem(f32, i);
    nanChecks.push(typeof v === "number" && Number.isNaN(v));
  }
  const negZero = readElem(f32, 4);
  const posInf = readElem(f32, 5);
  const negInf = readElem(f32, 6);
  const val7 = readElem(f32, 7);

  // Verify float32 store rounding inside DOUBLE region.
  const dst = new Float32Array(16);
  function scaleAdd(src, outArr) {
    for (let i = 0; i < outArr.length; i++) {
      outArr[i] = src[i & 7] + 0.1;
    }
  }
  for (let r = 0; r < 400; r++) scaleAdd(normal, dst);
  const f32RoundOk = dst[0] === Math.fround(0.25 + 0.1) && dst[3] === Math.fround(3.25 + 0.1);
  out.push(
    "f32=" +
      warmSum +
      ":" +
      nanChecks.join(",") +
      ":" +
      Object.is(negZero, -0) +
      ":" +
      (posInf === Infinity) +
      ":" +
      (negInf === -Infinity) +
      ":" +
      val7 +
      ":" +
      f32RoundOk
  );
}

// 2. pinned_nullish_cmp (WebforAI affine pattern + !== null + undefined/null bailout)
{
  const affine = (w, bias, input, inputOffset, outArr) => {
    const { rows, cols, values } = w;
    for (let row = 0, base = 0; row < rows; row++, base += cols) {
      let sum = bias === void 0 ? 0 : bias[row];
      for (let col = 0; col < cols; col++) {
        sum += values[base + col] * input[inputOffset + col];
      }
      outArr[row] = sum;
    }
  };
  const affineNull = (w, bias, input, outArr) => {
    const { rows, cols, values } = w;
    for (let row = 0, base = 0; row < rows; row++, base += cols) {
      let sum = bias !== null ? bias[row] : 1;
      for (let col = 0; col < cols; col++) {
        sum += values[base + col] * input[col];
      }
      outArr[row] = sum;
    }
  };
  const w = { rows: 4, cols: 8, values: new Float32Array(32) };
  for (let i = 0; i < 32; i++) w.values[i] = (i % 5) * 0.5;
  const bias = new Float32Array([0.5, 1.0, 1.5, 2.0]);
  const bias2 = new Float32Array([10, 20, 30, 40]);
  const inp = new Float32Array([1, 2, 3, 4, 5, 6, 7, 8]);
  const res = new Float32Array(4);

  for (let r = 0; r < 600; r++) affine(w, bias, inp, 0, res);
  const withBias = Array.from(res).join(",");
  affine(w, undefined, inp, 0, res);
  const withUndef = Array.from(res).join(",");
  affine(w, bias2, inp, 0, res);
  const withBias2 = Array.from(res).join(",");

  for (let r = 0; r < 600; r++) affineNull(w, bias, inp, res);
  const nullNonNull = Array.from(res).join(",");
  affineNull(w, null, inp, res);
  const nullHit = Array.from(res).join(",");
  out.push("affine=" + withBias + "|" + withUndef + "|" + withBias2 + "|" + nullNonNull + "|" + nullHit);
}

// 3. global_route_epoch != 0 cross-call fast path + live shadowing invalidation
{
  // Defining an accessor on globalThis bumps global_route_epoch (like Node bootstrap).
  Object.defineProperty(globalThis, "__issue101_boot_lazy", {
    get() { return 1; },
    configurable: true,
  });
  globalThis.__issue101_g = 10;

  function pureLeaf(x) { return x + 1; }
  function readsGlobal(x) { return x + __issue101_g; }
  function caller(n) {
    let s = 0;
    for (let i = 0; i < n; i++) {
      s += pureLeaf(i) + readsGlobal(1);
    }
    return s;
  }
  const before = caller(500);
  // Shadow global via accessor mid-execution — cross-call must observe immediately.
  Object.defineProperty(globalThis, "__issue101_g", {
    get() { return 100; },
    configurable: true,
  });
  const after = caller(500);
  delete globalThis.__issue101_g;
  delete globalThis.__issue101_boot_lazy;
  out.push("global=" + before + "/" + after);
}

// 4. Arrow destructuring unboxing + TDZ & capture semantics
{
  const unboxed = (o) => {
    const { a, b } = o;
    let t = 0;
    for (let i = 0; i < 100; i++) t += a + b;
    return t;
  };
  const tdzObj = (o) => {
    const { a = b, b = 1 } = o;
    return a + b;
  };
  const tdzArr = (arr) => {
    const [x = y, y = 1] = arr;
    return x + y;
  };
  const captured = (o) => {
    let { a, b } = o;
    const inc = () => { a += b; return a; };
    return inc() + inc();
  };
  let e1 = "none", e2 = "none";
  try { tdzObj({}); } catch (e) { e1 = e.name; }
  try { tdzArr([]); } catch (e) { e2 = e.name; }
  out.push("arrow=" + unboxed({ a: 2, b: 3 }) + "/" + tdzObj({ a: 2 }) + "/" + e1 + "/" + e2 + "/" + captured({ a: 1, b: 4 }));
}

// 5. Class member lookup cache + Walked::ClassGetter route + mutation invalidation
{
  const dynSym = "dynMethod";
  class BaseNode {
    constructor(n) { this._n = n; }
    get val() { return this._n * 2; }
    tag() { return "base"; }
  }
  class ElemNode extends BaseNode {
    constructor(n, id) { super(n); this._id = id; }
    get label() { return this._id + ":" + this.val; }
    [dynSym]() { return this._id; }
  }
  const nodes = [];
  for (let i = 0; i < 40; i++) nodes.push(new ElemNode(i, "e" + i));
  function sumNodes(list) {
    let s = 0;
    let last = "";
    for (let r = 0; r < 60; r++) {
      for (let i = 0; i < list.length; i++) {
        const nd = list[i];
        s += nd.val + (nd.missing === undefined ? 1 : 0);
        last = nd.tag() + "/" + nd.label + "/" + nd[dynSym]();
      }
    }
    return s + ":" + last;
  }
  const c1 = sumNodes(nodes);
  // Mutate prototype method & getter mid-run.
  ElemNode.prototype.tag = function () { return "patched"; };
  Object.defineProperty(BaseNode.prototype, "val", {
    get() { return this._n + 10; },
    configurable: true,
  });
  const c2 = sumNodes(nodes);
  // Retarget single instance prototype.
  const lone = new ElemNode(5, "x");
  Object.setPrototypeOf(lone, { val: 77, tag() { return "alt"; }, label: "L", dynMethod() { return "D"; } });
  const c3 = lone.val + "/" + lone.tag() + "/" + lone.label;
  out.push("class=" + c1 + "|" + c2 + "|" + c3);
}

// 6. RegexpProtoFastSlots + non-global str.match(re) + sticky /y + invalidation
{
  const rePlain = /([a-z]+)(\d+)/;
  const reGlobal = /[a-z]+/g;
  const reSticky = /[a-z]+/y;
  const reSplit = /\s+/;
  const reRepl = /\d+/g;
  function runRx(n) {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const m = "foo123bar".match(rePlain);
      if (m) acc += m[1].length + m[2].length;
      const mg = "ab 12 cd".match(reGlobal);
      if (mg) acc += mg.length;
      reSticky.lastIndex = 0;
      const ms1 = "abc def".match(reSticky);
      const li1 = reSticky.lastIndex;
      const ms2 = "abc def".match(reSticky);
      const li2 = reSticky.lastIndex;
      acc += (ms1 ? ms1[0].length : 0) + li1 + (ms2 ? 100 : 0) + li2;
      acc += "a  b\tc".split(reSplit).length;
      acc += "x1y22".replace(reRepl, "#").length;
    }
    return acc;
  }
  const r1 = runRx(400);

  // Override exec on RegExp.prototype — non-global match & replace & split must observe.
  const origExec = RegExp.prototype.exec;
  let execHits = 0;
  RegExp.prototype.exec = function (s) {
    execHits++;
    return origExec.call(this, s);
  };
  const r2 = runRx(5);
  RegExp.prototype.exec = origExec;

  // Override flags getter on RegExp.prototype — non-global match must observe.
  const origFlagsDesc = Object.getOwnPropertyDescriptor(RegExp.prototype, "flags");
  let flagHits = 0;
  Object.defineProperty(RegExp.prototype, "flags", {
    get() { flagHits++; return origFlagsDesc.get.call(this); },
    configurable: true,
  });
  const mFlag = "foo123".match(rePlain);
  Object.defineProperty(RegExp.prototype, "flags", origFlagsDesc);

  // Override global getter on RegExp.prototype.
  const origGlobalDesc = Object.getOwnPropertyDescriptor(RegExp.prototype, "global");
  let globalHits = 0;
  Object.defineProperty(RegExp.prototype, "global", {
    get() { globalHits++; return origGlobalDesc.get.call(this); },
    configurable: true,
  });
  const mGlob = "foo123".match(rePlain);
  Object.defineProperty(RegExp.prototype, "global", origGlobalDesc);

  out.push(
    "rx=" +
      r1 +
      "/" +
      r2 +
      "/" +
      (execHits > 0) +
      "/" +
      (flagHits > 0) +
      "/" +
      (globalHits > 0) +
      "/" +
      (mFlag[0] === "foo123") +
      "/" +
      (mGlob[0] === "foo123")
  );
}

// 7. Pooled String / Array / Closure fast paths (edge cases + churn)
{
  // fromCodePoint / fromCharCode
  let sAcc = 0;
  for (let i = 0; i < 600; i++) {
    const c1 = String.fromCodePoint(65 + (i % 26));
    const c2 = String.fromCharCode(97 + (i % 26));
    const empty = String.fromCodePoint();
    const astral = String.fromCodePoint(0x1f600);
    sAcc += c1.length + c2.length + empty.length + astral.length;
  }
  const badCps = [-1, 0x110000, 1.5, NaN];
  const cpErrs = badCps.map((v) => {
    try { String.fromCodePoint(v); return "ok"; } catch (e) { return e.name; }
  });
  const negCharCode = String.fromCharCode(-1).charCodeAt(0);

  // slice / substring / trim* / toLowerCase / toUpperCase / split
  let strRes = "";
  for (let i = 0; i < 400; i++) {
    const base = "  Hello, World!  ";
    const t = base.trim();
    const sl = t.slice(0, 5);
    const sub = t.substring(7, 12);
    const one = t.slice(0, 1);
    const empty = t.slice(5, 5);
    const whole = t.slice(0);
    const lo = t.toLowerCase();
    const up = t.toUpperCase();
    const parts = "alpha beta  gamma".split(" ");
    const longStr = "A".repeat(80);
    const longLo = longStr.toLowerCase();
    const astral = "\u{1F600}abc\u{1F600}".slice(2, 5);
    const trimEmpty = "   \t\n ".trim();
    const trimOne = "  x  ".trim();
    const oneLo = "Z".toLowerCase();
    const oneUp = "z".toUpperCase();
    if (i === 399) {
      strRes = [
        t, sl, sub, one, empty.length, whole === t, lo, up,
        parts.join("|"), longLo.length, astral, trimEmpty.length,
        trimOne, oneLo, oneUp
      ].join(",");
    }
  }

  // array.map / filter / slice + capturing closure churn (upval_pool)
  let arrSum = 0;
  for (let i = 0; i < 500; i++) {
    const k = i & 7;
    const base = [1, 2, 3, 4, 5, 6];
    const mapped = base.map((x) => x + k);
    const filt = mapped.filter((x) => (x & 1) === 0);
    const sliced = filt.slice(0, 2);
    for (let j = 0; j < sliced.length; j++) arrSum += sliced[j];
  }
  out.push("pool=" + sAcc + "|" + cpErrs.join(",") + "|" + negCharCode + "|" + strRes + "|" + arrSum);
}

console.log(out.join("\n"));
"####;

const EXPECTED: &str = "\
f32=1500:true,true,true,true:true:true:true:1.5:true
affine=32,36.5,38.5,33|31.5,35.5,37,31|41.5,55.5,67,71|32,36.5,38.5,33|32.5,36.5,38,32
global=130750/175750
arrow=500/3/ReferenceError/ReferenceError/14
class=96000:base/e39:78/e39|73200:patched/e39:49/e39|77/alt/L
rx=8400/105/true/true/true/true/true
pool=2400|RangeError,RangeError,RangeError,RangeError|65535|Hello, World!,Hello,World,H,0,true,hello, world!,HELLO, WORLD!,alpha|beta||gamma,80,abc,0,x,z,Z|5984";

#[test]
fn issue_101_probe_child() {
    if std::env::var_os("ZIPP_ISSUE_101_CHILD").is_none() {
        return;
    }
    let res = run(PROBE_PROGRAM).expect("probe compiles");
    assert_eq!(res.error, None, "runtime error: {:?} {:?}", res.error, res.errput);
    let actual = res.output.join("\n");
    assert_eq!(actual, EXPECTED);
}

#[test]
fn issue_101_all_modes_exact() {
    if std::env::var_os("ZIPP_ISSUE_101_CHILD").is_some() {
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    for (label, env) in [
        ("interpreter", Some(("ZIPP_NOJIT", "1"))),
        ("default-thresholds", None),
        ("compile-at-once", Some(("ZIPP_JIT_THRESHOLD", "1"))),
        ("gc-stress", Some(("ZIPP_GC_STRESS", "1"))),
    ] {
        let mut cmd = Command::new(&exe);
        cmd.args(["--exact", "issue_101_probe_child", "--nocapture"])
            .env("ZIPP_ISSUE_101_CHILD", "1")
            .env_remove("ZIPP_NOJIT")
            .env_remove("ZIPP_JIT_THRESHOLD")
            .env_remove("ZIPP_GC_STRESS");
        if let Some((k, v)) = env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn issue_101 child");
        assert!(
            out.status.success(),
            "{label} failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn arrow_destructuring_unboxes_without_cells() {
    let src = r#"
        "use strict";
        const affine = (w, bias, input, out) => {
            const { rows, cols, values } = w;
            for (let r = 0; r < rows; r++) {
                let s = bias === void 0 ? 0 : bias[r];
                for (let c = 0; c < cols; c++) s += values[r * cols + c] * input[c];
                out[r] = s;
            }
        };
        const out = new Float32Array(2);
        affine({ rows: 2, cols: 2, values: new Float32Array([1, 2, 3, 4]) }, undefined, new Float32Array([5, 6]), out);
        console.log(out[0], out[1]);
    "#;
    let bc = compile_to_text(src, false).expect("compiles");
    assert!(
        !bc.contains("MakeCellTdz"),
        "uncaptured arrow destructuring locals stayed boxed:\n{bc}"
    );
    let res = run(src).expect("runs");
    assert_eq!(res.error, None);
    assert_eq!(res.output, ["17 39"]);
}
