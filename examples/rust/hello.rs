#!/usr/bin/env rust-script
//! Example Rust script running directly on the ZIPP VM (`zipp rs examples/rust/hello.rs`).

use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
struct Task {
    id: usize,
    title: String,
    done: bool,
}

impl Task {
    fn new(id: usize, title: &str) -> Self {
        Self {
            id,
            title: title.to_string(),
            done: false,
        }
    }

    fn complete(&mut self) {
        self.done = true;
    }
}

fn fib(n: i64) -> i64 {
    if n <= 1 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    println!("ZIPP Rust script! args={:?}", args);

    let mut tasks = vec![
        Task::new(1, "Parse Rust without rustc"),
        Task::new(2, "Compile directly to ZIPP bytecode"),
        Task::new(3, "Run backend APIs on the ZIPP VM"),
    ];
    tasks[0].complete();
    tasks[1].complete();

    let done_count = tasks.iter().filter(|t| t.done).count();
    println!("Completed {done_count}/{} tasks", tasks.len());

    let mut freqs: BTreeMap<String, i32> = BTreeMap::new();
    for word in "zipp runs js py php rust on one vm runs fast".split_whitespace() {
        *freqs.entry(word.to_string()).or_insert(0) += 1;
    }
    println!("'runs' count = {}", freqs["runs"]);
    println!("fib(25) = {} (in {:.2} ms)", fib(25), start.elapsed().as_secs_f64() * 1000.0);
}

#[test]
fn test_fib() {
    assert_eq!(fib(10), 55);
    assert_eq!(fib(20), 6765);
}
