#![cfg(feature = "python")]
use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);
/// Windows' clock granularity is coarse enough that two tests starting in the
/// same tick produced the SAME directory, so one run saw the other's files.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "zipp-python-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn run(&self, entry: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_zipp"))
            .current_dir(&self.0)
            .args(["py", entry])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn writes_empty_files_deletes_and_renames() {
    let f = Fixture::new();
    fs::write(f.0.join("main.py"), "import os\nopen('empty.txt', 'w').close()\nopen('truncate.txt', 'w').close()\nos.remove('delete.txt')\nos.rename('rename.txt', 'renamed.txt')\n").unwrap();
    for name in ["truncate.txt", "delete.txt", "rename.txt"] {
        fs::write(f.0.join(name), "before").unwrap();
    }
    let output = f.run("main.py");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["empty.txt", "truncate.txt"] {
        assert_eq!(fs::read(f.0.join(name)).unwrap(), b"");
    }
    for name in ["delete.txt", "rename.txt"] {
        assert!(!f.0.join(name).exists());
    }
    assert_eq!(fs::read(f.0.join("renamed.txt")).unwrap(), b"before");
}

#[test]
fn all_python_extensions_keep_project_imports_and_data() {
    for entry in ["main.pyw", "MAIN.PY", "Main.PYW"] {
        let f = Fixture::new();
        fs::write(
            f.0.join(entry),
            "import helper\nprint(helper.value, open('data.txt').read())\n",
        )
        .unwrap();
        fs::write(f.0.join("helper.PY"), "value = 42\n").unwrap();
        fs::write(f.0.join("data.txt"), "project").unwrap();
        let output = f.run(entry);
        assert!(
            output.status.success(),
            "{entry}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42 project");
    }
}

#[test]
fn nested_entry_keeps_its_sibling_imports() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("example")).unwrap();
    fs::write(
        f.0.join("example/main.py"),
        "import helper\nprint(helper.value)\n",
    )
    .unwrap();
    fs::write(f.0.join("example/helper.py"), "value = 42\n").unwrap();
    let output = f.run("example/main.py");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn stdout_and_stderr_preserve_exact_bytes_without_appended_newline() {
    let cases: &[(&str, &str, &[u8], &[u8])] = &[
        ("print_no_end", "print('hello', end='')\n", b"hello", b""),
        (
            "stdout_write",
            "import sys\nassert sys.stdout.write('hello') == 5\n",
            b"hello",
            b"",
        ),
        ("print_default_nl", "print('hello')\n", b"hello\n", b""),
        (
            "adjacent_writes",
            "print('a', end='')\nprint('b', end='', flush=True)\n",
            b"ab",
            b"",
        ),
        (
            "stderr_print_no_end",
            "import sys\nprint('err', end='', file=sys.stderr)\n",
            b"",
            b"err",
        ),
        (
            "stderr_write",
            "import sys\nassert sys.stderr.write('err') == 3\n",
            b"",
            b"err",
        ),
        (
            "astral_codepoint_count",
            "import sys\nassert sys.stdout.write('🦀') == 1\n",
            "🦀".as_bytes(),
            b"",
        ),
        (
            "stdout_none_no_output",
            "import sys\nsys.stdout = None\nprint('hidden')\n",
            b"",
            b"",
        ),
        (
            "stdout_redirection",
            "import io, sys\nbuf = io.StringIO()\norig = sys.stdout\nsys.stdout = buf\nprint('captured', end='', flush=True)\nsys.stdout = orig\nsys.stdout.write(buf.getvalue())\n",
            b"captured",
            b"",
        ),
    ];
    for (name, src, want_out, want_err) in cases {
        let f = Fixture::new();
        fs::write(f.0.join("main.py"), src).unwrap();
        let output = f.run("main.py");
        assert!(
            output.status.success(),
            "{name}: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, *want_out, "{name}: stdout mismatch");
        assert_eq!(output.stderr, *want_err, "{name}: stderr mismatch");
    }
}

#[test]
fn stdout_and_stderr_do_not_truncate_past_64_kib() {
    // Regression for #108: large writes must stream byte-for-byte without
    // truncating at 64 KiB with `...\n`.
    let f = Fixture::new();

    fs::write(
        f.0.join("write_70k.py"),
        "import sys\nn = sys.stdout.write('a' * 70000)\nassert n == 70000\n",
    )
    .unwrap();
    let out = f.run("write_70k.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 70_000);
    assert!(out.stdout.iter().all(|&b| b == b'a'));
    assert!(out.stderr.is_empty());

    fs::write(f.0.join("print_64k_no_nl.py"), "print('a' * 65536, end='')\n").unwrap();
    let out = f.run("print_64k_no_nl.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 65_536);
    assert!(out.stdout.iter().all(|&b| b == b'a'));
    assert!(out.stderr.is_empty());

    fs::write(f.0.join("print_70k_nl.py"), "print('a' * 70000)\n").unwrap();
    let out = f.run("print_70k_nl.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 70_001);
    assert!(out.stdout[..70_000].iter().all(|&b| b == b'a'));
    assert_eq!(out.stdout[70_000], b'\n');
    assert!(out.stderr.is_empty());

    fs::write(
        f.0.join("stderr_70k.py"),
        "import sys\nn = sys.stderr.write('b' * 70000)\nassert n == 70000\n",
    )
    .unwrap();
    let out = f.run("stderr_70k.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr.len(), 70_000);
    assert!(out.stderr.iter().all(|&b| b == b'b'));

    fs::write(
        f.0.join("buffer_binary_70k.py"),
        "import sys\npayload = b'\\x00' + b'x' * 70000 + b'\\xff'\nassert sys.stdout.buffer.write(payload) == 70002\nsys.stdout.buffer.flush()\n",
    )
    .unwrap();
    let out = f.run("buffer_binary_70k.py");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 70_002);
    assert_eq!(out.stdout[0], 0x00);
    assert!(out.stdout[1..70_001].iter().all(|&b| b == b'x'));
    assert_eq!(out.stdout[70_001], 0xff);
    assert!(out.stderr.is_empty());
}

#[test]
fn stdin_mode_preserves_exact_bytes_and_streams_past_64_kib() {
    use std::io::Write;
    use std::process::Stdio;

    for (src, want) in [
        ("print('hello', end='')\n", b"hello".to_vec()),
        ("import sys\nsys.stdout.write('a' * 70000)\n", vec![b'a'; 70_000]),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_zipp"))
            .args(["py", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(src.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, want);
        assert!(output.stderr.is_empty());
    }
}
