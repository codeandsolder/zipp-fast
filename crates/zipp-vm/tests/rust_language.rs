#![cfg(feature = "rust")]

use std::collections::HashMap;
use zipp_vm::frontend::{compile_rust_program, compile_source, Frontend, RustMode, RustProgramConfig};

fn run_rs(src: &str) -> String {
    let mut compiled = compile_source(
        src,
        Frontend::Rust {
            mode: RustMode::Script,
        },
    )
    .expect("rust compile");
    compiled.state_mut().run_init().expect("rust run");
    let bytes = compiled.state_mut().take_rust_output();
    String::from_utf8(bytes).expect("utf8 output")
}

fn run_rs_cfg(src: &str, cfg: RustProgramConfig) -> (Result<(), String>, String, String) {
    let mut compiled = compile_rust_program(src, cfg).expect("rust compile");
    let res = compiled.state_mut().run_init().map(|_| ());
    let out = String::from_utf8_lossy(&compiled.state_mut().take_rust_output()).into_owned();
    let err = String::from_utf8_lossy(&compiled.state_mut().take_rust_stderr()).into_owned();
    (res, out, err)
}

#[test]
fn arithmetic_division_casts_and_shadowing() {
    let out = run_rs(
        r#"
        fn main() {
            let x: i32 = 7;
            let y: i32 = 2;
            let fx: f64 = 7.0;
            let fy: f64 = 2.0;
            println!("{} {} {}", x / y, -x / y, fx / fy);
            let x = x * 10;
            let x = x + 5;
            println!("shadow={x} u8={} char={}", 300i32 as u8, 65u32 as char);
            println!("bits={} {} {}", (0b1010 & 0b1100), (1 << 4), (32 >> 2));
        }
        "#,
    );
    assert_eq!(out, "3 -3 3.5\nshadow=75 u8=44 char=A\nbits=8 16 8\n");
}

#[test]
fn structs_enums_traits_impls_and_derives() {
    let out = run_rs(
        r#"
        #[derive(Debug, Clone, PartialEq, Default)]
        struct Point {
            x: i32,
            y: i32,
        }

        struct Wrapper(i32);

        trait Describe {
            fn label(&self) -> String;
            fn banner(&self) -> String {
                format!("[{}]", self.label())
            }
        }

        impl Point {
            fn new(x: i32, y: i32) -> Self {
                Self { x, y }
            }
            fn translate(&mut self, dx: i32, dy: i32) {
                self.x += dx;
                self.y += dy;
            }
        }

        impl Describe for Point {
            fn label(&self) -> String {
                format!("({}, {})", self.x, self.y)
            }
        }

        #[derive(Debug, PartialEq)]
        enum Shape {
            Unit,
            Circle(f64),
            Rect { w: i32, h: i32 },
        }

        impl Shape {
            fn area(&self) -> f64 {
                match self {
                    Shape::Unit => 0.0,
                    Shape::Circle(r) => 3.0 * r * r,
                    Shape::Rect { w, h } => (*w * *h) as f64,
                }
            }
        }

        fn main() {
            let mut p = Point::new(3, 4);
            let p2 = Point { x: 10, ..p };
            p.translate(1, -2);
            let w = Wrapper(99);
            println!("{} {} {:?} eq={}", p.banner(), p2.label(), w, p == Point::new(4, 2));
            let s1 = Shape::Circle(2.0);
            let s2 = Shape::Rect { w: 4, h: 5 };
            println!("areas={} {} {}", Shape::Unit.area(), s1.area(), s2.area());
        }
        "#,
    );
    assert_eq!(
        out,
        "[(4, 2)] (10, 4) Wrapper(99) eq=true\nareas=0 12 20\n"
    );
}

#[test]
fn pattern_matching_control_flow_and_let_else() {
    let out = run_rs(
        r#"
        fn classify(n: i32) -> &'static str {
            match n {
                -10..=-1 => "neg",
                0 => "zero",
                1 | 2 | 3 => "tiny",
                k @ 4..=10 if k % 2 == 0 => "even-mid",
                4..=10 => "odd-mid",
                _ => "big",
            }
        }

        fn unwrap_or_neg(opt: Option<i32>) -> i32 {
            let Some(v) = opt else { return -1; };
            v * 2
        }

        fn main() {
            println!("{} {} {} {} {} {}",
                classify(-5), classify(0), classify(2),
                classify(8), classify(7), classify(99));

            let arr = vec![10, 20, 30, 40, 50];
            match arr {
                [first, mid @ .., last] => println!("slice={first},{:?},{last}", mid),
                _ => println!("no"),
            }

            let found = 'outer: loop {
                for i in 0..10 {
                    if i == 6 {
                        break 'outer i * 7;
                    }
                }
            };
            let a = Some(10);
            let b = Some(5);
            let chained = if let Some(x) = a && x > 5 && let Some(y) = b {
                x + y
            } else {
                0
            };
            println!("loop={found} chain={chained} let_else={} {}", unwrap_or_neg(Some(6)), unwrap_or_neg(None));
        }
        "#,
    );
    assert_eq!(
        out,
        "neg zero tiny even-mid odd-mid big\nslice=10,[20, 30, 40],50\nloop=42 chain=15 let_else=12 -1\n"
    );
}

#[test]
fn mutable_refs_closures_and_move_semantics() {
    let out = run_rs(
        r#"
        use std::mem;

        fn bump(x: &mut i32) {
            *x += 5;
        }

        fn main() {
            let mut a = 10;
            let mut b = 20;
            bump(&mut a);
            mem::swap(&mut a, &mut b);
            println!("a={a} b={b}");

            let mut acc = 0;
            let nums = vec![1, 2, 3, 4, 5];
            nums.iter().for_each(|x| { acc += x; });
            println!("acc={acc}");

            // `move` closures inside a loop capture independent copies
            let mut fns = Vec::new();
            for i in 0..4 {
                fns.push(move |k: i32| i * 10 + k);
            }
            let res: Vec<i32> = fns.iter().map(|f| f(1)).collect();
            println!("moved={:?}", res);

            let mut opt = Some(42);
            let taken = opt.take();
            println!("taken={:?} left={:?}", taken, opt);
        }
        "#,
    );
    assert_eq!(
        out,
        "a=20 b=15\nacc=15\nmoved=[1, 11, 21, 31]\ntaken=Some(42) left=None\n"
    );
}

#[test]
fn strings_iterators_collections_and_entry_api() {
    let out = run_rs(
        r#"
        use std::collections::{BTreeMap, HashMap, HashSet, VecDeque, BinaryHeap};

        fn main() {
            let mut s = String::from("hello");
            s.push_str(" café");
            // UTF-8 byte length: "hello caf" (9) + "é" (2) = 11
            println!("len={} upper={} rev={}",
                s.len(),
                "rust".to_uppercase(),
                "abc".chars().rev().collect::<String>());

            let words = "the quick brown fox jumps over the lazy fox the";
            let mut counts: BTreeMap<String, i32> = BTreeMap::new();
            for w in words.split_whitespace() {
                *counts.entry(w.to_string()).or_insert(0) += 1;
            }
            println!("the={} fox={} unique={}", counts["the"], counts["fox"], counts.len());

            let evens: Vec<i32> = (1..=10).filter(|x| x % 2 == 0).map(|x| x * x).collect();
            println!("evens={:?} sum={}", evens, evens.iter().sum::<i32>());

            let mut map = HashMap::from([("a", 1), ("b", 2)]);
            map.insert("c", 3);
            let mut set = HashSet::from([1, 2, 3]);
            set.insert(2);
            let mut dq = VecDeque::new();
            dq.push_back(2);
            dq.push_front(1);
            dq.push_back(3);
            let mut heap = BinaryHeap::from([3, 1, 5, 2]);
            println!("map={} set={} dq={:?} top={:?}", map.len(), set.len(), dq.pop_front(), heap.pop());
        }
        "#,
    );
    assert_eq!(
        out,
        "len=11 upper=RUST rev=cba\nthe=3 fox=2 unique=7\nevens=[4, 16, 36, 64, 100] sum=220\nmap=3 set=3 dq=Some(1) top=Some(5)\n"
    );
}

#[test]
fn try_operator_async_await_macros_and_smart_pointers() {
    let out = run_rs(
        r#"
        use std::sync::{Arc, Mutex};
        use std::sync::atomic::{AtomicUsize, Ordering};

        macro_rules! sum_all {
            ($($x:expr),* $(,)?) => { 0 $(+ $x)* };
        }

        fn parse_double(s: &str) -> Result<i64, String> {
            let n = s.parse::<i64>()?;
            Ok(n * 2)
        }

        async fn compute(v: i64) -> i64 {
            let inner = async move { v + 10 }.await;
            inner * 2
        }

        #[tokio::main]
        async fn main() -> Result<(), String> {
            let a = compute(5).await;
            let (b, c) = tokio::join!(compute(1), compute(2));
            let shared = Arc::new(Mutex::new(Vec::new()));
            let cloned = Arc::clone(&shared);
            cloned.lock().unwrap().push(a);
            shared.lock().unwrap().push(b + c);
            let atom = AtomicUsize::new(10);
            let prev = atom.fetch_add(5, Ordering::SeqCst);
            println!("async={a},{b},{c} mutex={:?} atom={prev}->{}",
                *shared.lock().unwrap(), atom.load(Ordering::SeqCst));
            println!("try={:?} {:?} macro={} matches={}",
                parse_double("21"), parse_double("nope").is_err(),
                sum_all!(1, 2, 3, 4), matches!(a, 20..=40));
            Ok(())
        }
        "#,
    );
    assert_eq!(
        out,
        "async=30,22,24 mutex=[30, 46] atom=10->15\ntry=Ok(42) true macro=10 matches=true\n"
    );
}

#[test]
fn multifile_modules_via_vfs_and_test_runner() {
    let mut vfs = HashMap::new();
    vfs.insert(
        "math.rs".to_owned(),
        br#"
        pub const FACTOR: i32 = 3;
        pub fn triple(x: i32) -> i32 { x * FACTOR }
        "#
        .to_vec(),
    );
    let main_src = r#"
        mod math;
        use crate::math::triple;

        #[test]
        fn test_triple() {
            assert_eq!(triple(7), 21);
        }

        #[test]
        #[should_panic]
        fn test_panic() {
            panic!("boom");
        }

        #[test]
        #[ignore]
        fn test_skip() {
            panic!("should not run");
        }

        fn main() {
            println!("triple={}", triple(14));
        }
    "#;

    let (res, out, _) = run_rs_cfg(
        main_src,
        RustProgramConfig {
            file: "main.rs".to_owned(),
            base_dir: "".to_owned(),
            vfs: Some(vfs.clone()),
            mode: "run".to_owned(),
            ..Default::default()
        },
    );
    assert!(res.is_ok());
    assert_eq!(out, "triple=42\n");

    let (res, test_out, _) = run_rs_cfg(
        main_src,
        RustProgramConfig {
            file: "main.rs".to_owned(),
            base_dir: "".to_owned(),
            vfs: Some(vfs),
            mode: "test".to_owned(),
            ..Default::default()
        },
    );
    assert!(res.is_ok());
    assert!(test_out.contains("test test_triple ... ok"), "{test_out}");
    assert!(test_out.contains("test test_panic ... ok"), "{test_out}");
    assert!(test_out.contains("test test_skip ... ignored"), "{test_out}");
    assert!(test_out.contains("test result: ok. 2 passed; 0 failed; 1 ignored"), "{test_out}");
}

#[test]
fn jit_hot_compute_fib_and_loop() {
    let out = run_rs(
        r#"
        fn fib(n: i64) -> i64 {
            if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
        }

        fn sum_loop(n: i64) -> i64 {
            let mut acc: i64 = 0;
            for i in 1..=n {
                acc += i;
            }
            acc
        }

        fn main() {
            println!("fib(25)={} sum(100000)={}", fib(25), sum_loop(100_000));
        }
        "#,
    );
    assert_eq!(out, "fib(25)=75025 sum(100000)=5000050000\n");
}

#[test]
fn from_into_error_conversion_and_try_operator() {
    let out = run_rs(
        r#"
        #[derive(Debug)]
        struct IoErr { msg: String }

        #[derive(Debug)]
        struct AuthErr { code: i32 }

        #[derive(Debug)]
        enum AppError {
            Io(String),
            Auth(i32),
            Other(String),
        }

        impl From<IoErr> for AppError {
            fn from(e: IoErr) -> Self {
                AppError::Io(e.msg)
            }
        }

        impl From<AuthErr> for AppError {
            fn from(e: AuthErr) -> Self {
                AppError::Auth(e.code)
            }
        }

        impl From<String> for AppError {
            fn from(s: String) -> Self {
                AppError::Other(s)
            }
        }

        #[derive(Debug)]
        struct EvenNum(i32);

        impl TryFrom<i32> for EvenNum {
            type Error = String;
            fn try_from(n: i32) -> Result<Self, String> {
                if n % 2 == 0 {
                    Ok(EvenNum(n))
                } else {
                    Err(format!("odd: {n}"))
                }
            }
        }

        fn fail_io() -> Result<i32, IoErr> {
            Err(IoErr { msg: String::from("disk") })
        }

        fn fail_auth() -> Result<i32, AuthErr> {
            Err(AuthErr { code: 403 })
        }

        fn run_step(kind: i32) -> Result<i32, AppError> {
            if kind == 1 {
                let v = fail_io()?;
                return Ok(v);
            }
            if kind == 2 {
                let v = fail_auth()?;
                return Ok(v);
            }
            if kind == 3 {
                let _: i32 = Err(String::from("bad"))?;
            }
            Ok(42)
        }

        fn main() {
            println!("r0={:?} r1={:?} r2={:?} r3={:?}",
                run_step(0), run_step(1), run_step(2), run_step(3));
            let direct: AppError = AuthErr { code: 401 }.into();
            let via_from = AppError::from(IoErr { msg: String::from("eof") });
            println!("direct={:?} from={:?}", direct, via_from);
            println!("tf={:?} {:?}", EvenNum::try_from(8), EvenNum::try_from(5));
        }
        "#,
    );
    assert_eq!(
        out,
        "r0=Ok(42) r1=Err(Io(\"disk\")) r2=Err(Auth(403)) r3=Err(Other(\"bad\"))\ndirect=Auth(401) from=Io(\"eof\")\ntf=Ok(EvenNum(8)) Err(\"odd: 5\")\n"
    );
}

#[test]
fn mut_self_enum_state_machine_custom_iterator_and_boxed_closures() {
    let out = run_rs(
        r#"
        #[derive(Debug, PartialEq)]
        enum JobState {
            Idle,
            Running(i32),
            Done(i32),
        }

        impl JobState {
            fn step(&mut self) {
                let next = match self {
                    JobState::Idle => JobState::Running(1),
                    JobState::Running(n) => {
                        if *n >= 3 {
                            JobState::Done(*n)
                        } else {
                            JobState::Running(*n + 1)
                        }
                    }
                    JobState::Done(n) => JobState::Done(*n),
                };
                *self = next;
            }
        }

        struct Counter {
            cur: i32,
            max: i32,
        }

        impl Iterator for Counter {
            type Item = i32;
            fn next(&mut self) -> Option<i32> {
                if self.cur < self.max {
                    let v = self.cur;
                    self.cur += 1;
                    Some(v)
                } else {
                    None
                }
            }
        }

        fn apply(f: Box<dyn Fn(i32) -> i32>, v: i32) -> i32 {
            f(v)
        }

        fn main() {
            let mut st = JobState::Idle;
            st.step();
            st.step();
            st.step();
            st.step();
            println!("state={:?}", st);

            let c = Counter { cur: 1, max: 10 };
            let evens: Vec<i32> = c.filter(|x| x % 2 == 0).map(|x| x * 10).take(3).collect();
            println!("iter={:?}", evens);

            let adder: Box<dyn Fn(i32) -> i32> = Box::new(|x| x + 1);
            println!("boxed={} applied={}", adder(41), apply(Box::new(|x| x * 3), 7));
        }
        "#,
    );
    assert_eq!(
        out,
        "state=Done(3)\niter=[20, 40, 60]\nboxed=42 applied=21\n"
    );
}

#[test]
fn serde_attributes_nested_types_and_generics() {
    let out = run_rs(
        r##"
        use serde::{Serialize, Deserialize};

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tag {
            tag_id: i64,
            label: String,
        }

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct User {
            user_id: i64,
            #[serde(rename = "full_name")]
            display_name: String,
            #[serde(skip_serializing)]
            password_hash: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            bio: Option<String>,
            tags: Vec<Tag>,
        }

        fn main() {
            let u = User {
                user_id: 7,
                display_name: String::from("Ada Lovelace"),
                password_hash: String::from("SECRET_HASH"),
                bio: None,
                tags: vec![Tag { tag_id: 1, label: String::from("math") }],
            };
            let json_str = serde_json::to_string(&u).unwrap();
            println!("ser={json_str}");
            assert!(!json_str.contains("SECRET_HASH"));
            assert!(!json_str.contains("bio"));
            assert!(json_str.contains("\"userId\":7"));
            assert!(json_str.contains("\"full_name\":\"Ada Lovelace\""));
            assert!(json_str.contains("\"tagId\":1"));

            let arr_json = r#"[{"userId":9,"full_name":"Grace Hopper","bio":"COBOL","tags":[{"tagId":2,"label":"compiler"}]}]"#;
            let users: Vec<User> = serde_json::from_str::<Vec<User>>(arr_json).unwrap();
            println!("de={} bio={:?} tag0={}:{}",
                users[0].display_name, users[0].bio, users[0].tags[0].tag_id, users[0].tags[0].label);
        }
        "##,
    );
    assert!(out.contains("de=Grace Hopper bio=Some(\"COBOL\") tag0=2:compiler\n"), "{out}");
}

#[cfg(not(feature = "safe-sandbox"))]
#[test]
fn cargo_project_lib_crate_reexports_cfg_test_and_async_tests() {
    let tmp = std::env::temp_dir().join(format!("zipp_rs_cargo_proj_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("src/models")).unwrap();

    std::fs::write(
        tmp.join("Cargo.toml"),
        r#"
[package]
name = "demo-pkg"
version = "1.2.3"
edition = "2021"
"#,
    )
    .unwrap();

    std::fs::write(
        tmp.join("src/models/item.rs"),
        r#"
#[derive(Debug, Clone)]
pub struct Item {
    pub title: String,
    pub score: i64,
}

impl Item {
    pub fn new(title: &str, score: i64) -> Self {
        Self { title: title.to_string(), score }
    }
}
"#,
    )
    .unwrap();

    std::fs::write(
        tmp.join("src/models/mod.rs"),
        r#"
pub mod item;
pub use item::Item;
"#,
    )
    .unwrap();

    std::fs::write(
        tmp.join("src/lib.rs"),
        r#"
pub mod models;

pub fn summarize(item: &crate::models::Item) -> String {
    format!("{}:{}", item.title, item.score)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fetch_bonus() -> i64 {
        5
    }

    #[test]
    fn test_sync_summary() {
        let it = crate::models::Item::new("alpha", 10);
        assert_eq!(summarize(&it), "alpha:10");
    }

    #[test]
    async fn test_async_bonus() {
        let b = fetch_bonus().await;
        let it = crate::models::Item::new("beta", 10 + b);
        assert_eq!(summarize(&it), "beta:15");
    }
}
"#,
    )
    .unwrap();

    let main_src = r#"
use demo_pkg::models::Item;
use demo_pkg::summarize;

fn main() {
    let it = Item::new("launch", 99);
    println!("pkg={}@{} item={}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        summarize(&it));
}
"#;
    std::fs::write(tmp.join("src/main.rs"), main_src).unwrap();

    let main_path = tmp.join("src/main.rs").to_string_lossy().replace('\\', "/");
    let src_dir = tmp.join("src").to_string_lossy().replace('\\', "/");

    let (res, out, err) = run_rs_cfg(
        main_src,
        RustProgramConfig {
            file: main_path.clone(),
            base_dir: src_dir.clone(),
            allow_native_fs: true,
            mode: "run".to_owned(),
            ..Default::default()
        },
    );
    assert!(res.is_ok(), "run failed: {res:?} stderr={err}");
    assert_eq!(out, "pkg=demo-pkg@1.2.3 item=launch:99\n");

    let (res, test_out, test_err) = run_rs_cfg(
        main_src,
        RustProgramConfig {
            file: main_path,
            base_dir: src_dir,
            allow_native_fs: true,
            mode: "test".to_owned(),
            ..Default::default()
        },
    );
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(res.is_ok(), "test failed: {res:?} out={test_out} err={test_err}");
    assert!(test_out.contains("test_sync_summary ... ok"), "{test_out}");
    assert!(test_out.contains("test_async_bonus ... ok"), "{test_out}");
    assert!(test_out.contains("2 passed; 0 failed"), "{test_out}");
}

#[test]
fn extra_macros_crypto_jwt_time_html_and_bufreader() {
    let out = run_rs(
        r#"
        use std::io::{BufReader, Cursor};

        fn check_positive(n: i32) -> Result<i32, String> {
            ensure!(n > 0, "must be positive, got {}", n);
            if n > 100 {
                bail!("too large: {}", n);
            }
            Ok(n)
        }

        fn main() {
            println!("chk={:?} {:?} {:?}",
                check_positive(5), check_positive(-1), check_positive(200));

            // Password hashing & JWT HS256
            let hash = crypto::hash_password("s3cr3t!");
            let ok1 = crypto::verify_password("s3cr3t!", &hash);
            let ok2 = crypto::verify_password("wrong", &hash);
            let claims = serde_json::json!({ "sub": "u42", "role": "admin" });
            let tok = crypto::jwt_sign(&claims, "my-secret-key").unwrap();
            let verified = crypto::jwt_verify(&tok, "my-secret-key").unwrap();
            let bad_sig = crypto::jwt_verify(&tok, "wrong-key").is_err();
            println!("pw={ok1},{ok2} jwt_sub={} bad_sig={bad_sig}",
                verified["sub"].as_str().unwrap());

            // Utc::now, html::escape, BufReader<Cursor>
            let ts = Utc::now().to_rfc3339();
            assert!(ts.ends_with('Z') && ts.contains('T'));
            let esc = html::escape("<b>a & \"b\"</b>");
            let reader = BufReader::new(Cursor::new("line1\nline2\nline3"));
            let lines: Vec<String> = reader.lines().map(|r| r.unwrap()).collect();
            println!("esc={esc} lines={:?}", lines);
        }
        "#,
    );
    assert_eq!(
        out,
        "chk=Ok(5) Err(\"must be positive, got -1\") Err(\"too large: 200\")\npw=true,false jwt_sub=u42 bad_sig=true\nesc=&lt;b&gt;a &amp; &quot;b&quot;&lt;/b&gt; lines=[\"line1\", \"line2\", \"line3\"]\n"
    );
}
