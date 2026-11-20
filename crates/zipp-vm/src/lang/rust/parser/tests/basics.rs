use zipp_rsparse::{parse, Limits};

#[test]
fn parses_structs_enums_impls_traits_generics() {
    let src = r#"
        #![allow(dead_code)]
        use std::collections::{HashMap, HashSet as Set};
        use crate::models::*;

        #[derive(Debug, Clone, PartialEq)]
        pub struct User<T> {
            pub id: u64,
            pub name: String,
            pub meta: Option<Vec<T>>,
        }

        pub enum Command {
            Quit,
            Move { x: i32, y: i32 },
            Write(String),
        }

        pub trait Greet: Clone + Send {
            fn greet(&self) -> String {
                format!("hi")
            }
        }

        impl<T> User<T> {
            pub fn new(id: u64, name: String) -> Self {
                Self { id, name, meta: None }
            }
        }
    "#;
    let krate = parse(src, Limits::DEFAULT).expect("parse ok");
    assert_eq!(krate.inner_attrs.len(), 1);
    assert_eq!(krate.items.len(), 6);
}

#[test]
fn parses_control_flow_patterns_closures_async_macros() {
    let src = r##"#!/usr/bin/env rust-script
        macro_rules! say {
            ($x:expr) => { println!("{}", $x); };
        }

        async fn fetch_user(id: i64) -> Result<String, String> {
            let pair = ((1, 2), 3);
            let nested = pair.0.1;
            let raw = r#"hello "world""#;
            let ch = 'a';
            let byte_s = b"abc\x00";
            let mut sum = 0i64;
            'outer: for i in 0..=10 {
                if i == 5 { continue 'outer; }
                sum += i;
            }
            let x = match nested {
                0 | 1 => "small",
                n @ 2..=5 if n > 1 => "mid",
                _ => "big",
            };
            let v = vec![1, 2, 3].into_iter().map(|n| n * 2).collect::<Vec<i32>>();
            let _ = (raw, ch, byte_s, sum, x, v);
            let val = async { Ok::<String, String>(format!("user-{id}")) }.await?;
            Ok(val)
        }
    "##;
    let krate = parse(src, Limits::DEFAULT).expect("parse ok");
    assert_eq!(krate.items.len(), 2);
}

#[test]
fn parses_let_else_and_if_let_chains() {
    let src = r#"
        fn check(a: Option<i32>, b: Option<i32>) -> i32 {
            let Some(x) = a else { return 0; };
            if let Some(y) = b && y > 0 && x > 0 {
                x + y
            } else {
                x
            }
        }
    "#;
    let krate = parse(src, Limits::DEFAULT).expect("parse ok");
    assert_eq!(krate.items.len(), 1);
}

#[test]
fn parses_block_stmts_followed_by_deref_or_unary_or_tuple() {
    let src = r#"
        fn update(x: &mut i32, ok: bool) -> (i32, i32) {
            if !ok {
                return (0, 0);
            }
            *x = 10;
            match *x {
                10 => { *x += 1; }
                -1 => { *x = 0; }
                _ => {}
            }
            (*x, -1)
        }
    "#;
    let krate = parse(src, Limits::DEFAULT).expect("parse ok");
    assert_eq!(krate.items.len(), 1);
}
