//! PHP frontend entry: compiles PHP 8.x source + memoized JS runtime seed into a ZIPP `Program`,
//! plus OPcache-backed dynamic compilation for `include`/`require`/`eval` and HTTP SAPI configuration.

mod emitter;
mod exprs;
mod fast_int;
mod resolve;
mod stmts;
mod symtable;

use crate::bytecode::{FuncProto, Instr, PhpCtx, Program};
use emitter::{FileCtx, FuncSink, PhpEmitter};
use resolve::NsResolver;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use symtable::FnScopeInfo;
use zipp_phpparse::{parse, Limits};

type R<T> = Result<T, String>;

macro_rules! phpsrc {
    ($path:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/phpsrc/", $path))
    };
}

const RUNTIME_JS: &str = concat!(
    phpsrc!("runtime/core.js"),
    "\n",
    phpsrc!("runtime/array.js"),
    "\n",
    phpsrc!("runtime/oop.js"),
    "\n",
    phpsrc!("runtime/stdlib.js"),
    "\n",
    phpsrc!("runtime/spl_reflection.js"),
    "\n",
    phpsrc!("runtime/sapi.js"),
    "\n",
    phpsrc!("runtime/entry.js"),
);

const MAX_SOURCE: usize = 4 * 1024 * 1024;

#[derive(Clone)]
struct RuntimeSeed {
    program: Program,
    entry_fn: usize,
    rt_slot: u32,
}

static RUNTIME_SEED: OnceLock<Result<RuntimeSeed, String>> = OnceLock::new();

fn build_runtime_seed() -> R<RuntimeSeed> {
    let mut program = crate::compile_only(RUNTIME_JS, false)
        .map_err(|e| format!("PHP runtime compile error: {e}"))?;
    program.php_natives = true;
    let entry_fn = program
        .functions
        .iter()
        .position(|f| f.name == "__zipp_php_entry")
        .ok_or("PHP runtime: missing __zipp_php_entry")?;
    let rt_slot = program
        .global_names
        .iter()
        .position(|n| n == "__zipp_php")
        .ok_or("PHP runtime: missing __zipp_php")? as u32;
    Ok(RuntimeSeed {
        program,
        entry_fn,
        rt_slot,
    })
}

fn runtime_seed() -> R<RuntimeSeed> {
    RUNTIME_SEED
        .get_or_init(build_runtime_seed)
        .clone()
}

/// Configuration for compiling and running a PHP script or HTTP request.
#[derive(Clone, Debug, Default)]
pub struct PhpProgramConfig {
    /// Logical/real script file path (for `__FILE__`, `__DIR__`, relative `include`).
    pub file: String,
    /// Base directory for resolving relative includes and filesystem operations.
    pub base_dir: String,
    /// Optional CLI arguments (`argv`, with `argv[0]` = script path).
    pub argv: Vec<String>,
    /// Optional HTTP SAPI request JSON (`{ sapi, server, query, cookies, contentType, body, post, files }`).
    pub sapi_request_json: Option<String>,
    /// Optional virtual filesystem map (used in sandboxed / WASM / embedded runs).
    pub vfs: Option<HashMap<String, Vec<u8>>>,
    /// Whether real host filesystem access is allowed (true in CLI/server, false in sandbox/WASM).
    pub allow_native_fs: bool,
    /// Whether raw PHP output writes directly to OS stdout (true in CLI script mode; false in HTTP SAPI / tests).
    pub stream_stdout: bool,
}

/// Compile a standalone PHP source string (default config, buffered output, no native FS).
pub(in crate::lang) fn compile(source: &str) -> R<Program> {
    compile_program(
        source,
        PhpProgramConfig {
            file: "<php>".to_owned(),
            base_dir: ".".to_owned(),
            allow_native_fs: false,
            stream_stdout: false,
            ..Default::default()
        },
    )
}

/// Compiled entry-program cache keyed by `(file, base_dir)` + source stamp, so repeated
/// HTTP requests in `zipp php -S` skip lexing, parsing, and emitting the entry script!
static ENTRY_CACHE: OnceLock<Mutex<HashMap<(String, String), (u64, Program, u32)>>> =
    OnceLock::new();

fn entry_cache() -> &'static Mutex<HashMap<(String, String), (u64, Program, u32)>> {
    ENTRY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Compile a PHP entry program with full [`PhpProgramConfig`] (file path, argv, SAPI request, VFS/FS).
pub(crate) fn compile_program(source: &str, config: PhpProgramConfig) -> R<Program> {
    if source.len() > MAX_SOURCE {
        return Err("PHP: source exceeds 4 MiB limit".into());
    }
    let dir = if config.base_dir.is_empty() {
        parent_dir_of(&config.file)
    } else {
        config.base_dir.clone()
    };
    let stamp = source_stamp(source);
    let cache_key = (config.file.clone(), dir.clone());
    let cached = if !config.file.is_empty() && config.file != "<php>" && config.file != "test.php" {
        let cache = entry_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache
            .get(&cache_key)
            .filter(|(s, _, _)| *s == stamp)
            .map(|(_, p, rt)| (p.clone(), *rt))
    } else {
        None
    };

    let (mut program, rt_slot) = if let Some(hit) = cached {
        hit
    } else {
        let ast = parse(source, Limits::DEFAULT).map_err(|e| e.to_string())?;
        let seed = runtime_seed()?;
        let program = seed.program;
        let entry_fn = seed.entry_fn;
        let rt_slot = seed.rt_slot;

        let name_global = program.functions[entry_fn].name_global;
        let file_ctx = FileCtx::new(&config.file, &dir, source);

        let prog_cell = RefCell::new(program);
        let sink = FuncSink::Program(&prog_cell);
        let scope_info = FnScopeInfo {
            use_scope_map: true,
            ..Default::default()
        };
        let mut emitter = PhpEmitter::new(
            "__zipp_php_entry",
            rt_slot,
            &sink,
            file_ctx,
            NsResolver::default(),
            None,
            None,
            String::new(),
            &scope_info,
            &[],
            &[],
            true,
            false,
        )?;
        emitter.emit_top_level(&ast.stmts)?;
        let ret = emitter.null()?;
        emitter.emit(Instr::Return { src: ret })?;
        emitter.patch_gotos()?;
        let mut entry_proto = emitter.finish();
        entry_proto.name_global = name_global;

        let mut program = prog_cell.into_inner();
        program.functions.replace(entry_fn, entry_proto);
        program.php_natives = true;
        if !config.file.is_empty() && config.file != "<php>" && config.file != "test.php" {
            let mut cache = entry_cache().lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() > 256 {
                cache.clear();
            }
            cache.insert(cache_key, (stamp, program.clone(), rt_slot));
        }
        (program, rt_slot)
    };

    program.php_ctx = Some(Box::new(PhpCtx {
        file: config.file,
        base_dir: dir,
        rt_slot,
        argv: config.argv,
        sapi_request_json: config.sapi_request_json,
        vfs: config.vfs.map(Arc::new),
        allow_native_fs: config.allow_native_fs,
        stream_stdout: config.stream_stdout,
        compile_unit: compile_dynamic_unit_cached,
    }));
    Ok(program)
}

// ---- In-process OPcache for dynamically included `.php` files ----

#[derive(Clone)]
struct OpcacheEntry {
    stamp: u64,
    /// Compiled `FuncProto`s with relative function IDs starting at 0; top-level body is LAST.
    protos: Arc<[FuncProto]>,
}

static OPCACHE: OnceLock<Mutex<HashMap<String, OpcacheEntry>>> = OnceLock::new();

fn opcache() -> &'static Mutex<HashMap<String, OpcacheEntry>> {
    OPCACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn source_stamp(source: &str) -> u64 {
    // Fast FNV-1a 64-bit hash + length for exact content validation.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (source.len() as u64);
    for &b in source.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Compile a dynamically `include`/`require`/`eval`'d PHP unit into `FuncProto`s shifted
/// to `base_func_id`. The top-level file body is the LAST proto in the returned `Vec`.
pub(crate) fn compile_dynamic_unit_cached(
    cache_key: Option<&str>,
    file_path: &str,
    source: &str,
    rt_slot: u32,
    base_func_id: u32,
) -> R<Vec<FuncProto>> {
    if source.len() > MAX_SOURCE {
        return Err("PHP: included file exceeds 4 MiB limit".into());
    }
    let stamp = source_stamp(source);
    if let Some(key) = cache_key {
        let cache = opcache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(key) {
            if entry.stamp == stamp {
                return Ok(relocate_protos(&entry.protos, base_func_id));
            }
        }
    }

    let ast = parse(source, Limits::DEFAULT).map_err(|e| format!("{file_path}: {e}"))?;
    let dir = parent_dir_of(file_path);
    let file_ctx = FileCtx::new(file_path, &dir, source);

    let protos_cell = RefCell::new(Vec::<FuncProto>::new());
    let sink = FuncSink::Dynamic {
        base_id: 0, // compile at base 0 so OPcache entries are relocatable!
        protos: &protos_cell,
    };
    let scope_info = FnScopeInfo {
        use_scope_map: true,
        ..Default::default()
    };
    let mut emitter = PhpEmitter::new(
        file_path,
        rt_slot,
        &sink,
        file_ctx,
        NsResolver::default(),
        None,
        None,
        String::new(),
        &scope_info,
        &[],
        &[],
        true,
        false,
    )?;
    emitter.emit_top_level(&ast.stmts)?;
    // Included files return `1` by default in PHP unless they explicitly `return $val;`.
    let one = emitter.int64(1)?;
    emitter.emit(Instr::Return { src: one })?;
    emitter.patch_gotos()?;
    let top_proto = emitter.finish();

    let mut protos = protos_cell.into_inner();
    protos.push(top_proto);

    let shared: Arc<[FuncProto]> = protos.into();
    if let Some(key) = cache_key {
        let mut cache = opcache().lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > 2048 {
            cache.clear();
        }
        cache.insert(
            key.to_owned(),
            OpcacheEntry {
                stamp,
                protos: Arc::clone(&shared),
            },
        );
    }

    Ok(relocate_protos(&shared, base_func_id))
}

fn relocate_protos(protos: &[FuncProto], base_func_id: u32) -> Vec<FuncProto> {
    let mut out = protos.to_vec();
    if base_func_id == 0 {
        return out;
    }
    for p in &mut out {
        for ins in &mut p.code {
            match ins {
                Instr::MakeFunc { func_id, .. } | Instr::MakeClosure { func_id, .. } => {
                    *func_id += base_func_id;
                }
                _ => {}
            }
        }
    }
    out
}

fn parent_dir_of(path: &str) -> String {
    let norm = path.replace('\\', "/");
    match norm.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(i) => norm[..i].to_owned(),
        None => ".".to_owned(),
    }
}
