//! Python projects: modules and imports, the built-in `ui` module, and the
//! host hooks an embedder drives through global slots.
#![cfg(feature = "python")]
use zipp_vm::embed::{HostValue, ScriptState};
use zipp_vm::frontend::{compile_python_project, CompiledSource};

fn project(entry: &str, files: &[(&str, &str)]) -> Result<CompiledSource, String> {
    let modules: Vec<(String, String)> = files
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    compile_python_project(entry, &modules)
}

/// Runs on the CLI's stack size: dunder dispatch re-enters the interpreter
/// natively, and debug frames are large.
fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("test thread")
}

fn run_project(entry: &str, files: &[(&str, &str)]) -> Result<Vec<String>, String> {
    let entry = entry.to_owned();
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    big_stack(move || {
        let mut compiled = compile_python_project(&entry, &files)?;
        let state = compiled.state_mut();
        state.set_limits(20_000_000, None);
        state.run_init()?;
        Ok(state.take_output())
    })
}

fn slot(state: &ScriptState, name: &str) -> u32 {
    state
        .symbols()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.index)
        .unwrap_or_else(|| panic!("runtime hook {name} has no global slot"))
}

fn s(v: &str) -> HostValue {
    HostValue::String(v.to_string())
}

fn n(v: f64) -> HostValue {
    HostValue::Number(v)
}

#[test]
fn modules_import_each_other_with_separate_namespaces() {
    let main = "import util\nfrom util import double, NAME as label\nimport util as u\nx = 1\n\
                print(util.double(20), double(4), label, u.x, x)\nutil.bump()\nutil.bump()\n\
                print(util.count, util.tally())\nprint(util)\n";
    let util = "x = 99\nNAME = 'util-name'\ncount = 0\ndef double(n):\n    return n * 2\n\
                def bump():\n    count = 1\n    return None\ndef tally():\n    return count\n\
                print('util loaded')\n";
    // `count` is read through the module's own namespace by `tally`, and a
    // function body's assignment is a local that never leaks anywhere.
    assert_eq!(
        run_project("main", &[("main", main), ("util", util)]).unwrap(),
        vec![
            "util loaded",
            "40 8 util-name 99 1",
            "0 0",
            "<module 'util' from 'util.py'>"
        ]
    );
}

#[test]
fn a_module_runs_once_and_cycles_resolve_partially_like_cpython() {
    let files = [
        ("main", "import a\nimport b\nprint(a.value, b.value)\n"),
        (
            "a",
            "print('a start')\nimport b\nvalue = 1\nprint('a end', b.value)\n",
        ),
        (
            "b",
            "print('b start')\nimport a\nvalue = 2\nprint('b end')\n",
        ),
    ];
    assert_eq!(
        run_project("main", &files).unwrap(),
        vec!["a start", "b start", "b end", "a end 2", "1 2"]
    );
    // Reading a name the partially initialised module has not bound yet is an
    // AttributeError, not a silent None.
    let files = [
        ("main", "import a\n"),
        ("a", "import b\nvalue = 1\n"),
        ("b", "import a\nprint(a.value)\n"),
    ];
    let err = run_project("main", &files).err().unwrap();
    assert!(err.contains("AttributeError"), "{err}");
}

#[test]
fn unknown_and_unsupported_imports_are_compile_errors() {
    // An unknown module compiles and raises ModuleNotFoundError where the
    // import runs, so an `except ImportError` guard can catch it.
    for (source, needle) in [
        ("import nowhere\n", "ModuleNotFoundError: No module named 'nowhere'"),
        ("from nowhere import x\n", "ModuleNotFoundError: No module named 'nowhere'"),
        ("import a.b\n", "ModuleNotFoundError: No module named 'a'"),
        ("import util.b\n", "ModuleNotFoundError: No module named 'util.b'"),
        ("from util import nothing\n", "ImportError: cannot import name 'nothing' from 'util'"),
    ] {
        let err = run_project("main", &[("main", source), ("util", "x = 1\n")])
            .err()
            .unwrap();
        assert!(err.contains(needle), "{source:?}: {err}");
    }
    assert_eq!(
        run_project(
            "main",
            &[(
                "main",
                "try:\n    import nowhere\nexcept ImportError as e:\n    nowhere = None\n    print(type(e).__name__, e.name, e)\n\
                 try:\n    from nowhere.sub import thing\nexcept ModuleNotFoundError:\n    thing = 0\n\
                 print(nowhere, thing)\n",
            )],
        )
        .unwrap(),
        vec![
            "ModuleNotFoundError nowhere No module named 'nowhere'",
            "None 0"
        ]
    );
    // An error raised on a line after a call into another module returned
    // cites the line that raised it, not the callee's last line.
    let err = run_project(
        "main",
        &[
            (
                "main",
                "import util\n\ndef f():\n    return util.make().missing\n\nf()\n",
            ),
            ("util", "class T:\n    pass\n\ndef make():\n    x = 1\n    return T()\n"),
        ],
    )
    .err()
    .unwrap();
    assert!(
        err.contains("has no attribute 'missing' (main.py:4)"),
        "{err}"
    );
    let err = run_project(
        "main",
        &[
            ("main", "import util\n\nutil.boom()\n"),
            ("util", "def boom():\n    x = 1\n    raise ValueError('inner')\n"),
        ],
    )
    .err()
    .unwrap();
    assert!(err.contains("ValueError: inner (util.py:3)"), "{err}");
    for (source, needle) in [
        (
            "def f():\n    from util import *\n",
            "import * only allowed at module level",
        ),
        ("from . import util\n", "relative"),
    ] {
        let err = project("main", &[("main", source), ("util", "x = 1\n")])
            .err()
            .unwrap();
        assert!(err.contains(needle), "{source:?}: {err}");
    }
    let err = project("main", &[("main", "x = 1\n"), ("bad-name", "")])
        .err()
        .unwrap();
    assert!(err.contains("not a valid module name"), "{err}");
    let err = project("app", &[("main", "x = 1\n")]).err().unwrap();
    assert!(err.contains("entry module"), "{err}");
    let err = project("main", &[("main", "x = 1\n"), ("main", "")])
        .err()
        .unwrap();
    assert!(err.contains("duplicate"), "{err}");
    // Diagnostics name the file (a module is compiled once something
    // imports it; an unrelated file with a syntax error costs nothing).
    assert!(project("main", &[("main", "x = 1\n"), ("util", "x = = 2\n")]).is_ok());
    let err = project("main", &[("main", "import util\n"), ("util", "x = = 2\n")])
        .err()
        .unwrap();
    assert!(err.contains("util.py:1:5"), "{err}");
    // A module may import from another and from the stdlib inside a function.
    assert_eq!(
        run_project(
            "main",
            &[
                (
                    "main",
                    "def f():\n    import util\n    from math import sqrt\n    return util.x + sqrt(16)\nprint(f())\n"
                ),
                ("util", "x = 1\n")
            ]
        )
        .unwrap(),
        vec!["5.0"]
    );
}

#[test]
fn ui_module_records_commands_and_host_hooks_call_the_entry() {
    big_stack(ui_module_body);
}

fn ui_module_body() {
    let source = "import ui\nfrom ui import rect\n\nhits = [0]\n\n\
                  def draw():\n    ui.clear('#000')\n    rect(1, 2, 3, 4, 'red')\n    \
                  ui.text(5, 6, 'hi ' + str(len(hits)), 'white')\n    return len(hits)\n\n\
                  def add(a, b):\n    hits[0] = hits[0] + 1\n    return [a + b, 'ok', True, None]\n\n\
                  def on_click(x, y):\n    return ui.button(0, 0, 10, 10, 'go')\n\n\
                  print(ui.mouse(), ui.width(), ui.key('ArrowUp'))\nui.circle(9, 9, 2, 'blue')\n";
    let mut compiled = project("main", &[("main", source)]).unwrap();
    let state = compiled.state_mut();
    state.set_limits(20_000_000, None);
    state.run_init().unwrap();
    assert_eq!(state.take_output(), vec!["(0, 0, False) 640 False"]);
    let (has, call, take_ui, set_input) = (
        slot(state, "__zipp_py_has"),
        slot(state, "__zipp_py_call"),
        slot(state, "__zipp_py_take_ui"),
        slot(state, "__zipp_py_set_input"),
    );
    // Top-level commands are buffered until the host drains them.
    let drained = state.call_slot(take_ui, &[]).unwrap();
    assert_eq!(
        drained,
        HostValue::Array(vec![HostValue::Array(vec![
            s("circle"),
            n(9.0),
            n(9.0),
            n(2.0),
            s("blue")
        ])])
    );
    assert_eq!(
        state.call_slot(take_ui, &[]).unwrap(),
        HostValue::Array(vec![])
    );
    assert_eq!(
        state.call_slot(has, &[s("draw")]).unwrap(),
        HostValue::Bool(true)
    );
    assert_eq!(
        state.call_slot(has, &[s("update")]).unwrap(),
        HostValue::Bool(false)
    );
    assert_eq!(
        state.call_slot(has, &[s("hits")]).unwrap(),
        HostValue::Bool(false)
    );
    // Host numbers arrive as Python ints and results cross back as host data.
    let result = state
        .call_slot(call, &[s("add"), HostValue::Array(vec![n(2.0), n(40.0)])])
        .unwrap();
    assert_eq!(
        result,
        HostValue::Array(vec![
            n(42.0),
            s("ok"),
            HostValue::Bool(true),
            HostValue::Null
        ])
    );
    assert_eq!(
        state
            .call_slot(call, &[s("draw"), HostValue::Array(vec![])])
            .unwrap(),
        n(1.0)
    );
    let frame = state.call_slot(take_ui, &[]).unwrap();
    let HostValue::Array(commands) = frame else {
        panic!("{frame:?}")
    };
    assert_eq!(commands.len(), 3);
    assert_eq!(
        commands[2],
        HostValue::Array(vec![s("text"), n(5.0), n(6.0), s("hi 1"), s("white")])
    );
    // Input drives `button`, `mouse` and `key`: a click inside the button rect.
    let input = "{\"mx\":4,\"my\":5,\"down\":true,\"clicked\":true,\
                 \"keys\":{\"ArrowUp\":true},\"w\":320,\"h\":200}";
    state.call_slot(set_input, &[s(input)]).unwrap();
    let clicked = state
        .call_slot(
            call,
            &[s("on_click"), HostValue::Array(vec![n(4.0), n(5.0)])],
        )
        .unwrap();
    assert_eq!(clicked, HostValue::Bool(true));
    // Errors from the hooks are ordinary throws with Python error names.
    let err = state
        .call_slot(call, &[s("missing"), HostValue::Array(vec![])])
        .err()
        .unwrap();
    assert!(err.contains("NameError"), "{err}");
    // Host floats arrive as Python floats.
    assert_eq!(
        state
            .call_slot(call, &[s("add"), HostValue::Array(vec![n(1.5), n(1.0)])])
            .unwrap(),
        HostValue::Array(vec![
            n(2.5),
            s("ok"),
            HostValue::Bool(true),
            HostValue::Null
        ])
    );
    let err = state
        .call_slot(call, &[s("add"), HostValue::Array(vec![n(1.0)])])
        .err()
        .unwrap();
    assert!(err.contains("positional argument"), "{err}");
}

#[test]
fn guest_python_cannot_reach_the_host_hooks_or_other_modules_privately() {
    for source in [
        "print(__zipp_py_ui)",
        "print(__zipp_py_call)",
        "__zipp_py_take_ui()",
        "print(util.x)",
        "print(main.x)",
    ] {
        assert!(
            run_project("main", &[("main", source), ("util", "x = 1\n")]).is_err(),
            "{source}"
        );
    }
}

#[test]
fn new_builtins_and_list_pop() {
    assert_eq!(
        run_project(
            "main",
            &[(
                "main",
                "a = [1, 2, 3]\nprint(a.pop(), a, min(3, 9), max('b', 'a'), int('42') + int(True))\n"
            )]
        )
        .unwrap(),
        vec!["3 [1, 2] 3 b 43"]
    );
    assert!(run_project("main", &[("main", "[].pop()")])
        .err()
        .unwrap()
        .contains("IndexError"));
    assert!(run_project("main", &[("main", "int('x')")])
        .err()
        .unwrap()
        .contains("ValueError"));
}

/// `compile_python_program`: packages by dotted name, the virtual filesystem
/// and `sys.argv`, as the CLI and the playground hand a folder over.
fn run_program(
    entry: &str,
    modules: &[(&str, &str)],
    files: &[(&str, &[u8])],
    argv: &[&str],
) -> Result<Vec<String>, String> {
    use zipp_vm::frontend::compile_python_program;
    let entry = entry.to_owned();
    let modules: Vec<(String, String)> = modules
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let files: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(n, b)| (n.to_string(), b.to_vec()))
        .collect();
    let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
    big_stack(move || {
        let mut compiled = compile_python_program(&entry, &modules, &files, &argv, false)?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok(state.take_output())
    })
}

#[test]
fn packages_files_and_argv_reach_the_program() {
    let main = "import sys, os, json\nfrom legacy.fast_memory import Memory\nimport legacy\n\
                from pkg import helper\nimport pkg.sub.deep as deep\nimport pkg\n\
                print(sys.argv)\nprint(Memory().name, legacy.fast_memory.Memory is Memory)\n\
                print(helper.answer(), deep.LEVEL, pkg.sub.deep.LEVEL)\n\
                print(sorted(os.listdir('data')), os.path.exists('data/cfg.json'), os.path.isdir('legacy'))\n\
                with open('data/cfg.json') as f:\n    cfg = json.load(f)\nprint(cfg['steps'] * 2)\n\
                with open('data/blob.bin', 'rb') as f:\n    blob = f.read()\nprint(len(blob), blob[:2], blob[-1])\n\
                with open('out/result.txt', 'w') as f:\n    f.write('done ' + sys.argv[2])\n\
                print(open('out/result.txt').read())\n";
    let modules = [
        ("main", main),
        ("legacy.fast_memory", "class Memory:\n    name = 'fast'\n"),
        ("pkg", "print('pkg init')\n"),
        ("pkg.helper", "def answer():\n    return 42\n"),
        ("pkg.sub.deep", "LEVEL = 3\n"),
        ("unused", "raise RuntimeError('never compiled')\n"),
    ];
    let blob: Vec<u8> = (0..=255u8).collect();
    let files: [(&str, &[u8]); 4] = [
        ("data/cfg.json", b"{\"steps\": 21}"),
        ("data/blob.bin", &blob),
        ("legacy/fast_memory.py", b"class Memory:\n    name = 'fast'\n"),
        ("main.py", main.as_bytes()),
    ];
    let out = run_program("main", &modules, &files, &["--steps", "7"]).unwrap();
    assert_eq!(
        out,
        vec![
            "pkg init",
            "['main.py', '--steps', '7']",
            "fast True",
            "42 3 3",
            "['blob.bin', 'cfg.json'] True True",
            "42",
            "256 b'\\x00\\x01' 255",
            "done 7",
        ]
    );
    // A missing file is a FileNotFoundError, and a folder is not a file.
    let err = run_program("main", &[("main", "open('nope.txt')\n")], &[], &[])
        .err()
        .unwrap();
    assert!(err.contains("FileNotFoundError"), "{err}");
    let err = run_program(
        "main",
        &[("main", "open('data')\n")],
        &[("data/x.txt", b"x")],
        &[],
    )
    .err()
    .unwrap();
    assert!(err.contains("IsADirectoryError"), "{err}");
}

#[test]
fn a_test_module_entry_runs_its_tests() {
    let source = "import pytest\n\ndef test_ok():\n    assert 1 + 1 == 2\n\n\
                  @pytest.mark.parametrize('n', [1, 2])\ndef test_param(n):\n    assert n > 0\n";
    let out = run_program("test_lab", &[("test_lab", source)], &[], &[]).unwrap();
    assert!(out.iter().any(|l| l.contains("3 passed")), "{out:?}");
    let failing = "def test_bad():\n    assert 1 == 2\n";
    let err = run_program("test_lab", &[("test_lab", failing)], &[], &[])
        .err()
        .unwrap();
    assert!(err.contains("SystemExit"), "{err}");
}

/// Relative imports, which a ported package is written with. A module that has
/// submodules counts from itself; anything else counts from its parent, and
/// each extra dot climbs one more level -- CPython's rule.
#[test]
fn relative_imports_resolve_against_the_current_package() {
    let out = run_project(
        "main",
        &[
            ("main", "import pkg.sub.leaf
print(pkg.sub.leaf.report())
"),
            ("pkg", "VALUE = 'package'
"),
            ("pkg.shared", "NAME = 'shared'
"),
            ("pkg.sub", "DEPTH = 'sub'
"),
            (
                "pkg.sub.leaf",
                concat!(
                    // one dot: this module's package, pkg.sub
                    "from .  import sibling
",
                    "from . import DEPTH
",
                    // two dots: pkg
                    "from ..shared import NAME
",
                    "from .. import VALUE
",
                    "def report():
",
                    "    return ' '.join([sibling.WHO, DEPTH, NAME, VALUE])
",
                ),
            ),
            ("pkg.sub.sibling", "WHO = 'sibling'
"),
        ],
    )
    .expect("run");
    assert_eq!(out, vec!["sibling sub shared package"]);
}

/// A package's own `__init__` counts from itself, not from its parent.
#[test]
fn a_package_module_counts_relative_imports_from_itself() {
    let out = run_project(
        "main",
        &["main", "import pkg
print(pkg.HELLO)
"]
            .chunks(2)
            .map(|c| (c[0], c[1]))
            .collect::<Vec<_>>()
            .as_slice()
            .iter()
            .copied()
            .chain([("pkg", "from .inner import HELLO
"), ("pkg.inner", "HELLO = 'from inner'
")])
            .collect::<Vec<_>>()
            .as_slice(),
    )
    .expect("run");
    assert_eq!(out, vec!["from inner"]);
}

/// Climbing past the top of the project is an error, not a silent miss.
#[test]
fn a_relative_import_beyond_the_top_level_is_refused() {
    let error = run_project(
        "main",
        &[
            ("main", "import pkg.leaf
"),
            ("pkg", "
"),
            ("pkg.leaf", "from ... import nothing
"),
        ],
    )
    .expect_err("should refuse");
    assert!(error.contains("beyond top-level package"), "{error}");
}

/// A module with no package above it cannot climb at all.
#[test]
fn a_relative_import_with_no_parent_package_is_refused() {
    let error = run_project("main", &[("main", "from . import anything
")])
        .expect_err("should refuse");
    assert!(error.contains("no known parent package"), "{error}");
}

/// `compile_python_script_files`: a deferred file is read when the program
/// first reads it, once, and never when the program does not. A reader that
/// answers nothing, or more than the per-file limit, reads as a missing file.
#[test]
fn deferred_files_are_read_on_first_use() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use zipp_vm::frontend::{compile_python_script_files, PythonFile};
    let main = "import os\n\
                print(os.path.exists('a.txt'), os.path.exists('gone.txt'), os.path.exists('nope.txt'))\n\
                print(sorted(os.listdir('.')))\n\
                print(open('a.txt').read(), open('a.txt').read())\n\
                print(os.path.getsize('b.bin'), open('b.bin', 'rb').read()[:3])\n\
                for name in ('gone.txt', 'big.bin'):\n    \
                    try:\n        open(name, 'rb')\n    \
                    except FileNotFoundError:\n        print('missing', name)\n\
                with open('a.txt', 'w') as f:\n    f.write('new')\n\
                print(open('a.txt').read())\n";
    let counts: Vec<Arc<AtomicUsize>> = (0..5).map(|_| Arc::default()).collect();
    let deferred = |k: usize, size: usize, answer: Option<Vec<u8>>| {
        let count = Arc::clone(&counts[k]);
        PythonFile::Deferred {
            size,
            read: Arc::new(move || {
                count.fetch_add(1, Ordering::Relaxed);
                answer.clone()
            }),
        }
    };
    let files = vec![
        ("a.txt".to_owned(), deferred(0, 3, Some(b"old".to_vec()))),
        ("b.bin".to_owned(), deferred(1, 4, Some(vec![1, 2, 3, 4]))),
        ("never.txt".to_owned(), deferred(2, 5, Some(b"never".to_vec()))),
        ("gone.txt".to_owned(), deferred(3, 1, None)),
        ("big.bin".to_owned(), deferred(4, 1, Some(vec![0; 8 * 1024 * 1024 + 1]))),
        ("main.py".to_owned(), PythonFile::Bytes(main.as_bytes().into())),
    ];
    let out = big_stack(move || {
        let mut compiled = compile_python_script_files(
            "main",
            "main.py",
            &[("main".to_owned(), main)],
            files,
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(
        out,
        vec![
            "True True False",
            "['a.txt', 'b.bin', 'big.bin', 'gone.txt', 'main.py', 'never.txt']",
            "old old",
            "4 b'\\x01\\x02\\x03'",
            "missing gone.txt",
            "missing big.bin",
            "new",
        ]
    );
    let reads: Vec<usize> = counts.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    // Read once however often the program opens them; never when it does not.
    assert_eq!(reads[..3], [1, 1, 0], "{reads:?}");
    assert!(reads[3] >= 1 && reads[4] >= 1, "{reads:?}");
}

#[test]
fn text_mode_universal_newline_translation() {
    let main = r#"
from pathlib import Path
import re

p = Path("sample.txt")
# Binary read is byte-exact.
assert p.read_bytes() == b"first\r\nsecond\rthird\nfourth"
with open("sample.txt", "rb") as f:
    assert f.read() == b"first\r\nsecond\rthird\nfourth"

# Default newline=None translates CRLF and CR to LF (Path.read_text and open).
assert p.read_text() == "first\nsecond\nthird\nfourth"
assert p.read_text(encoding="utf-8") == "first\nsecond\nthird\nfourth"
assert p.read_text(newline="") == "first\r\nsecond\rthird\nfourth"
assert re.findall(r"^[a-z]+$", p.read_text(), re.MULTILINE) == ["first", "second", "third", "fourth"]

# All five newline modes match CPython for read(), list(f), readline(), read(n).
cases = {
    None: (
        "first\nsecond\nthird\nfourth",
        ["first\n", "second\n", "third\n", "fourth"],
        ["first\n", "second", "\nthird\nfourth"],
    ),
    "": (
        "first\r\nsecond\rthird\nfourth",
        ["first\r\n", "second\r", "third\n", "fourth"],
        ["first\r", "\nsecon", "d\rthird\nfourth"],
    ),
    "\n": (
        "first\r\nsecond\rthird\nfourth",
        ["first\r\n", "second\rthird\n", "fourth"],
        ["first\r", "\nsecon", "d\rthird\nfourth"],
    ),
    "\r": (
        "first\r\nsecond\rthird\nfourth",
        ["first\r", "\nsecond\r", "third\nfourth"],
        ["first\r", "\nsecon", "d\rthird\nfourth"],
    ),
    "\r\n": (
        "first\r\nsecond\rthird\nfourth",
        ["first\r\n", "second\rthird\nfourth"],
        ["first\r", "\nsecon", "d\rthird\nfourth"],
    ),
}
for nl, (exp_all, exp_lines, exp_chunks) in cases.items():
    with open("sample.txt", "r", newline=nl) as f:
        assert f.read() == exp_all, (nl, "all")
    with open("sample.txt", "r", newline=nl) as f:
        assert list(f) == exp_lines, (nl, "iter")
    with open("sample.txt", "r", newline=nl) as f:
        assert f.readlines() == exp_lines, (nl, "readlines")
    with open("sample.txt", "r", newline=nl) as f:
        got = [f.read(6), f.read(6), f.read()]
        assert got == exp_chunks, (nl, "chunks", got, exp_chunks)

# Writing with explicit newline translates LF.
out_crlf = Path("out_crlf.txt")
n = out_crlf.write_text("a\nb\n", newline="\r\n")
assert n == 4
assert out_crlf.read_bytes() == b"a\r\nb\r\n"
assert out_crlf.read_text() == "a\nb\n"

out_cr = Path("out_cr.txt")
out_cr.write_text("a\nb\n", newline="\r")
assert out_cr.read_bytes() == b"a\rb\r"
assert out_cr.read_text() == "a\nb\n"

# Invalid newline and binary mode errors.
for bad in ["x", "\n\r"]:
    try:
        open("sample.txt", "r", newline=bad)
        assert False
    except ValueError:
        pass
try:
    open("sample.txt", "rb", newline="\n")
    assert False
except ValueError:
    pass

print("ok")
"#;
    let files = vec![
        (
            "sample.txt".to_owned(),
            b"first\r\nsecond\rthird\nfourth".to_vec(),
        ),
        ("main.py".to_owned(), main.as_bytes().to_vec()),
    ];
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &files,
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}

#[test]
fn zipfile_deflated_and_compresslevel_and_shutil() {
    let main = r#"
import zipfile, zlib, shutil, os, struct

# Exact issue #104 repro.
info = zipfile.ZipInfo("test.txt", (1980, 1, 1, 0, 0, 0))
info.compress_type = zipfile.ZIP_DEFLATED
with zipfile.ZipFile("probe.zip", "w") as z:
    z.writestr(info, b"hello world" * 1000, compresslevel=9)

with zipfile.ZipFile("probe.zip", "r") as z:
    got = z.getinfo("test.txt")
    assert got.compress_type == zipfile.ZIP_DEFLATED, got.compress_type
    assert got.file_size == 11000
    assert got.compress_size < 200, got.compress_size
    assert got.date_time == (1980, 1, 1, 0, 0, 0)
    assert z.read("test.txt") == b"hello world" * 1000
    assert z.testzip() is None

# Check raw ZIP header method field is 8 (DEFLATED), not 0 (STORED).
raw_zip = open("probe.zip", "rb").read()
assert raw_zip[:4] == b"PK\x03\x04"
assert struct.unpack("<H", raw_zip[8:10])[0] == zipfile.ZIP_DEFLATED

# ZipFile-level compression and compresslevel.
with zipfile.ZipFile("all_deflated.zip", "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as z:
    z.writestr("a.txt", "repeat " * 500)
    z.writestr("b.txt", b"stored override", compress_type=zipfile.ZIP_STORED)

with zipfile.ZipFile("all_deflated.zip", "r") as z:
    ia = z.getinfo("a.txt")
    ib = z.getinfo("b.txt")
    assert ia.compress_type == zipfile.ZIP_DEFLATED and ia.compress_size < ia.file_size
    assert ib.compress_type == zipfile.ZIP_STORED and ib.compress_size == ib.file_size
    assert z.read("a.txt") == ("repeat " * 500).encode("utf-8")
    assert z.read("b.txt") == b"stored override"

# Unsupported compression methods must fail rather than silently storing.
for bad_method in (zipfile.ZIP_BZIP2, zipfile.ZIP_LZMA):
    try:
        zipfile.ZipFile("bad.zip", "w", compression=bad_method)
        assert False, f"expected rejection for {bad_method}"
    except NotImplementedError:
        pass

# zlib roundtrip (zlib, raw deflate, gzip) and checksums.
payload = b"The quick brown fox jumps over the lazy dog. " * 100
for wb in (15, -15, 31):
    comp = zlib.compress(payload, level=9, wbits=wb)
    assert len(comp) < len(payload) // 5
    assert zlib.decompress(comp, wbits=wb) == payload
assert zlib.crc32(b"123456789") == 0xCBF43926
assert zlib.adler32(b"Wikipedia") == 0x11E60398

# shutil.copyfile
shutil.copyfile("probe.zip", "probe_copy.zip")
assert open("probe_copy.zip", "rb").read() == raw_zip

print("ok")
"#;
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &[("main.py".to_owned(), main.as_bytes().to_vec())],
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}

#[test]
fn re_astral_offset_scaling_and_semantics() {
    let main = r#"
import re

# Group spans, pos, endpos across astral characters.
s = "a🦀b(🦀c)d"
p = re.compile(r"\((.)(.)\)")
m = p.search(s)
assert m is not None
assert m.span() == (3, 7), m.span()
assert m.span(1) == (4, 5), m.span(1)
assert m.span(2) == (5, 6), m.span(2)
assert m.group(1) == "🦀" and m.group(2) == "c"
assert p.match(s, 3).span() == (3, 7)
assert p.match(s, 3, 7).span() == (3, 7)
assert p.match(s, 3, 6) is None
assert p.search(s, 4) is None

# Lone surrogates do not shift code-point offsets.
lone = "ab\ud800cd\udc00ef"
m2 = re.compile(r"ef").search(lone)
assert m2.span() == (6, 8), m2.span()

# Issue #107 repro: 8000 positions with a trailing astral char must stay linear (fast).
text = "a" * 8000 + "🦀"
pat = re.compile(r"(?:b)?'(?:[^'\\\n\r\t]|\\(?:[nrt\\0'\"]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\}))'")
count = sum(pat.match(text, i) is not None for i in range(len(text)))
assert count == 0
# Also match at the end after 8000 ASCII chars + astral char.
text2 = "a" * 8000 + "🦀'x'"
m3 = pat.match(text2, 8001)
assert m3 is not None and m3.span() == (8001, 8004) and m3.group(0) == "'x'"
print("ok")
"#;
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &[("main.py".to_owned(), main.as_bytes().to_vec())],
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}

#[test]
fn html_urllib_warnings_and_support_stdlib() {
    let main = r#"
import sys, warnings, html, html.entities, html.parser
import urllib.parse, urllib.error, urllib.request
import codecs, logging, copyreg, threading, binascii, base64, tempfile, keyword, re
import collections, collections.abc, typing, contextlib

# sys.version_info named tuple & comparisons
assert sys.version_info.major == 3 and sys.version_info.minor >= 12
assert sys.version_info >= (3, 12) and not (sys.version_info >= (3, 14))
assert isinstance(sys.warnoptions, list)

# warnings + catch_warnings
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    warnings.warn("hello warning", DeprecationWarning)
assert len(caught) == 1 and caught[0].category is DeprecationWarning
assert str(caught[0].message) == "hello warning"

# html + html.entities + html.parser
assert html.escape('<a href="x">&</a>') == '&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;'
assert html.unescape('&lt;a&gt; &mdash; &#65; &#x42;') == '<a> \u2014 A B'
assert html.entities.name2codepoint["mdash"] == 0x2014
assert html.entities.html5["amp;"] == "&"

events = []
class Collector(html.parser.HTMLParser):
    def handle_starttag(self, tag, attrs):
        events.append(("start", tag, attrs))
    def handle_endtag(self, tag):
        events.append(("end", tag))
    def handle_data(self, data):
        events.append(("data", data))

p = Collector()
p.feed('<p class="lead">Hi &amp; <b>bye</b></p>')
p.close()
assert events[0] == ("start", "p", [("class", "lead")]), events
assert ("data", "Hi & ") in events and ("start", "b", []) in events and ("data", "bye") in events

# urllib.parse
u = urllib.parse.urlparse("https://example.com:8080/a/b?x=1&y=two#frag")
assert u.scheme == "https" and u.hostname == "example.com" and u.port == 8080
assert u.path == "/a/b" and u.fragment == "frag"
assert urllib.parse.parse_qs(u.query) == {"x": ["1"], "y": ["two"]}
assert urllib.parse.urljoin("https://example.com/a/b", "../c") == "https://example.com/c"
assert urllib.parse.unquote(urllib.parse.quote("a b/c")) == "a b/c"
assert issubclass(urllib.error.HTTPError, urllib.error.URLError)

# collections.abc, UserDict, typing, keyword
assert isinstance("abc", collections.abc.Hashable)
assert not isinstance([], collections.abc.Hashable)
assert isinstance({}, collections.abc.Mapping)
assert isinstance("abc", typing.Sized)
ud = collections.UserDict(a=1)
ud["b"] = 2
assert len(ud) == 2 and ud["a"] == 1 and ud["b"] == 2
assert keyword.iskeyword("def") and not keyword.iskeyword("not_a_kw")

# bytes regexes & Python 3.11+ atomic/possessive syntax
bp = re.compile(b"charset=([a-z0-9-]+)", re.I)
bm = bp.search(b"Content-Type: text/html; CHARSET=utf-8;")
assert bm is not None and bm.group(1) == b"utf-8" and bm.span(1) == (33, 38)
assert re.findall(b"[a-z]+", b"ab 12 cd") == [b"ab", b"cd"]
assert re.sub(b"a+", b"X", b"baaac") == b"bXc"
ap = re.compile(r"(?>\r\n|[\n\r])([a-f0-9]{1,6}+)")
am = ap.search("\r\nff00")
assert am is not None and am.group(1) == "ff00"

# base64, binascii, codecs, tempfile, threading
enc = base64.b64encode(b"zipp!")
assert base64.b64decode(enc) == b"zipp!"
assert binascii.unhexlify(binascii.hexlify(b"\xde\xad")) == b"\xde\xad"
assert codecs.lookup("utf-8").name == "utf-8"
lock = threading.RLock()
with lock:
    with lock:
        pass
with tempfile.TemporaryDirectory() as td:
    assert isinstance(td, str) and len(td) > 0
print("ok")
"#;
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &[("main.py".to_owned(), main.as_bytes().to_vec())],
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}

#[test]
fn tomllib_shlex_datetime_unittest_mock_importlib_util() {
    let main = r#"
import importlib

# Dynamic import of all 6 modules from Issue #103 without static import statements first.
for name in ("tomllib", "shlex", "datetime", "unittest", "unittest.mock", "importlib.util"):
    m = importlib.import_module(name)
    assert m is not None, name

tomllib = importlib.import_module("tomllib")
shlex = importlib.import_module("shlex")
datetime = importlib.import_module("datetime")
unittest = importlib.import_module("unittest")
mock = importlib.import_module("unittest.mock")
iu = importlib.import_module("importlib.util")

# tomllib: tables, dotted keys, array of tables, dates, inline tables, error handling.
doc = tomllib.loads('''
title = "Policy"
[pkg]
name = "demo"
ports = [80, 443]
meta.owner = "ci"
[[items]]
id = 1
when = 2026-11-23T12:30:00Z
[[items]]
id = 2
opts = { fast = true, ratio = 0.5 }
''')
assert doc["title"] == "Policy"
assert doc["pkg"]["meta"]["owner"] == "ci"
assert doc["items"][0]["when"] == datetime.datetime(2026, 11, 23, 12, 30, 0, tzinfo=datetime.timezone.utc)
assert doc["items"][1]["opts"] == {"fast": True, "ratio": 0.5}
try:
    tomllib.loads("broken = [1, 2")
    assert False
except tomllib.TOMLDecodeError:
    pass

# shlex
assert shlex.split("cmd --arg 'two words' \"three\\nfour\"") == ["cmd", "--arg", "two words", "three\\nfour"]
assert shlex.quote("simple") == "simple"
assert shlex.quote("a'b c") == "'a'\"'\"'b c'"
assert shlex.join(["git", "commit", "-m", "hi there"]) == "git commit -m 'hi there'"

# datetime
d = datetime.date(2026, 5, 1)
t = datetime.time(14, 30, 0)
dt = datetime.datetime.combine(d, t, tzinfo=datetime.timezone.utc)
assert dt.isoformat() == "2026-05-01T14:30:00+00:00"
parsed = datetime.datetime.fromisoformat("2026-05-01T16:30:00+02:00")
assert parsed == dt
assert (dt + datetime.timedelta(days=2, hours=1)).day == 3
assert datetime.datetime.strptime("2026-05-01 09:15", "%Y-%m-%d %H:%M").minute == 15

# unittest + unittest.mock.patch.object
class Service:
    def fetch(self, key):
        return f"real:{key}"

class PolicyTest(unittest.TestCase):
    def test_mock_patch_object(self):
        svc = Service()
        with mock.patch.object(svc, "fetch", return_value="mocked") as m:
            self.assertEqual(svc.fetch("abc"), "mocked")
            m.assert_called_once_with("abc")
        self.assertEqual(svc.fetch("abc"), "real:abc")

    def test_raises_and_subtest(self):
        with self.assertRaisesRegex(ValueError, "bad"):
            raise ValueError("bad input")
        for x in (2, 4, 6):
            with self.subTest(x=x):
                self.assertEqual(x % 2, 0)

suite = unittest.defaultTestLoader.loadTestsFromTestCase(PolicyTest)
res = unittest.TestResult()
suite.run(res)
assert res.wasSuccessful() and res.testsRun == 2, (res.failures, res.errors)

# importlib.util.spec_from_file_location loading a VFS file
spec = iu.spec_from_file_location("policy_mod", "scripts/policy_mod.py")
assert spec is not None and spec.loader is not None
mod = iu.module_from_spec(spec)
spec.loader.exec_module(mod)
assert mod.ANSWER == 42 and mod.greet("zipp") == "hello zipp"
print("ok")
"#;
    let helper = "ANSWER = 42\ndef greet(who):\n    return f'hello {who}'\n";
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &[
                ("main.py".to_owned(), main.as_bytes().to_vec()),
                ("scripts/policy_mod.py".to_owned(), helper.as_bytes().to_vec()),
            ],
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}

#[test]
fn re_findall_fastpath_and_pathlib_glob() {
    let main = r#"
import re
from pathlib import Path

# 0 groups, 1 group, 2+ groups, optional unmatched groups, empty matches, astral, pos/endpos.
assert re.compile(r"\d+").findall("a1b22c333") == ["1", "22", "333"]
assert re.compile(r"\[([^\]]+)\]").findall("[foo] [bar]") == ["foo", "bar"]
assert re.compile(r"(a)(b)?").findall("a ab") == [("a", ""), ("a", "b")]
assert re.compile(r"a*").findall("aba") == ["a", "", "a", ""]
assert re.compile(r"🦀(\w+)").findall("🦀one 🦀two") == ["one", "two"]
p = re.compile(r"\w+")
assert p.findall("one two three", 4, 7) == ["two"]

# pathlib glob / rglob + read_text
docs = sorted(Path("docs").rglob("*.md"))
assert [d.name for d in docs] == ["a.md", "b.md"]
assert [d.read_text(encoding="utf-8") for d in docs] == ["hello\n", "world\n"]
print("ok")
"#;
    let out = big_stack(move || {
        let mut compiled = zipp_vm::frontend::compile_python_program(
            "main",
            &[("main".to_owned(), main.to_owned())],
            &[
                ("main.py".to_owned(), main.as_bytes().to_vec()),
                ("docs/a.md".to_owned(), b"hello\r\n".to_vec()),
                ("docs/sub/b.md".to_owned(), b"world\r\n".to_vec()),
                ("docs/sub/skip.txt".to_owned(), b"ignore".to_vec()),
            ],
            &[],
            false,
        )?;
        let state = compiled.state_mut();
        state.set_limits(50_000_000, None);
        state.run_init()?;
        Ok::<_, String>(state.take_output())
    })
    .unwrap();
    assert_eq!(out, vec!["ok"]);
}
