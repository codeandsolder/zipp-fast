//! Rust frontend entry: compiles Rust source (including external `mod foo;` files)
//! + memoized JS runtime seed into a ZIPP `Program`.

#[cfg(not(any(target_arch = "wasm32", feature = "wasm-lite")))]
mod code_cache;
mod emitter;
mod exprs;
mod macros;
mod pat;
mod scope;

use crate::bytecode::{Instr, Program, RustCtx};
use emitter::{FileCtx, RsEmitter};
use scope::CrateSymbols;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use zipp_rsparse::ast::{Item, ItemKind};
use zipp_rsparse::{parse, Limits};

type R<T> = Result<T, String>;

macro_rules! rssrc {
    ($path:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/rssrc/", $path))
    };
}

const RUNTIME_JS: &str = concat!(
    rssrc!("runtime/core.js"),
    "\n",
    rssrc!("runtime/collections.js"),
    "\n",
    rssrc!("runtime/stdlib.js"),
    "\n",
    rssrc!("runtime/entry.js"),
);

const MAX_SOURCE: usize = 4 * 1024 * 1024;

#[derive(Clone)]
struct RuntimeSeed {
    program: Program,
    entry_fn: usize,
    rt_slot: u32,
}

static RUNTIME_MEMO: AtomicBool = AtomicBool::new(true);
static RUNTIME_SEED: OnceLock<Result<RuntimeSeed, String>> = OnceLock::new();

pub(crate) fn set_runtime_memo(enabled: bool) {
    RUNTIME_MEMO.store(enabled, Ordering::Relaxed);
}

pub(crate) fn set_runtime_cache(dir: Option<PathBuf>) {
    #[cfg(not(any(target_arch = "wasm32", feature = "wasm-lite")))]
    code_cache::set_dir(dir);
    #[cfg(any(target_arch = "wasm32", feature = "wasm-lite"))]
    let _ = dir;
}

fn build_runtime_seed() -> R<RuntimeSeed> {
    #[cfg(not(any(target_arch = "wasm32", feature = "wasm-lite")))]
    let mut program = code_cache::runtime(RUNTIME_JS)?;
    #[cfg(any(target_arch = "wasm32", feature = "wasm-lite"))]
    let mut program = crate::compile_only(RUNTIME_JS, false)
        .map_err(|e| format!("Rust runtime compile error: {e}"))?;
    program.rust_natives = true;
    let entry_fn = program
        .functions
        .iter()
        .position(|f| f.name == "__zipp_rs_entry")
        .ok_or("Rust runtime: missing __zipp_rs_entry")?;
    let rt_slot = program
        .global_names
        .iter()
        .position(|n| n == "__zipp_rs")
        .ok_or("Rust runtime: missing __zipp_rs")? as u32;
    Ok(RuntimeSeed {
        program,
        entry_fn,
        rt_slot,
    })
}

fn runtime_seed() -> R<RuntimeSeed> {
    if !RUNTIME_MEMO.load(Ordering::Relaxed) {
        return build_runtime_seed();
    }
    RUNTIME_SEED.get_or_init(build_runtime_seed).clone()
}

/// Configuration for compiling and running a Rust script, test suite, or backend service.
#[derive(Clone, Debug)]
pub struct RustProgramConfig {
    /// Logical or filesystem path of the entry `.rs` file.
    pub file: String,
    /// Base directory for resolving `mod foo;`, `include_str!`, and relative `std::fs` paths.
    pub base_dir: String,
    /// Command-line arguments (`std::env::args()`, with `argv[0]` = script path).
    pub argv: Vec<String>,
    /// Optional virtual filesystem map (used in sandboxed / WASM / embedded runs).
    pub vfs: Option<HashMap<String, Vec<u8>>>,
    /// Whether real host filesystem & env access is permitted (true in CLI/server, false in sandbox/WASM).
    pub allow_native_fs: bool,
    /// Whether real host TCP networking (`std::net`, `http::Router::serve`, `http::Client`) is permitted.
    pub allow_net: bool,
    /// Whether stdout/stderr streams directly to OS stdout/stderr (true in CLI; false in tests/worker).
    pub stream_stdout: bool,
    /// Execution mode: `"run"` (default), `"test"` (`#[test]` runner), or `"worker_boot"` (`zipp rs --serve`).
    pub mode: String,
}

impl Default for RustProgramConfig {
    fn default() -> Self {
        Self {
            file: "<rust>".to_owned(),
            base_dir: ".".to_owned(),
            argv: Vec::new(),
            vfs: None,
            allow_native_fs: false,
            allow_net: false,
            stream_stdout: false,
            mode: "run".to_owned(),
        }
    }
}

/// Compile a standalone Rust source string (default config, buffered output, no native FS/net).
pub(in crate::lang) fn compile(source: &str) -> R<Program> {
    compile_program(source, RustProgramConfig::default())
}

/// Compile a Rust program (single-file or multi-file `mod` project) into a ZIPP `Program`.
pub(crate) fn compile_program(source: &str, config: RustProgramConfig) -> R<Program> {
    if source.len() > MAX_SOURCE {
        return Err("Rust: source exceeds 4 MiB limit".into());
    }
    let dir = if config.base_dir.is_empty() {
        parent_dir_of(&config.file)
    } else {
        config.base_dir.clone()
    };

    let mut krate = parse(source, Limits::DEFAULT)
        .map_err(|e| e.format_with_source(source, &config.file))?;

    // Discover Cargo.toml (in `dir` or its parent if `dir` is `.../src`) for `CARGO_PKG_*` env
    // vars and automatic sibling `src/lib.rs` crate module mounting!
    let mut pkg_env = Vec::new();
    let mut lib_crate_name: Option<String> = None;
    let manifest = discover_cargo_manifest(Path::new(&dir), config.vfs.as_ref(), config.allow_native_fs);
    if let Some(ref m) = manifest {
        let crate_ident = m.name.replace('-', "_");
        pkg_env.push(("CARGO_PKG_NAME".to_owned(), m.name.clone()));
        pkg_env.push(("CARGO_PKG_VERSION".to_owned(), m.version.clone()));
        pkg_env.push(("CARGO_PKG_DESCRIPTION".to_owned(), m.description.clone()));
        pkg_env.push(("CARGO_CRATE_NAME".to_owned(), crate_ident.clone()));
        pkg_env.push(("CARGO_MANIFEST_DIR".to_owned(), m.manifest_dir.clone()));

        // If compiling a bin entry (e.g. `src/main.rs`) and `src/lib.rs` exists beside it,
        // mount `src/lib.rs` as `mod <crate_ident> { ... }` so `use my_app::...;` works cleanly!
        let file_name = Path::new(&config.file)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if file_name != "lib.rs" {
            if let Some((lib_src, lib_file, lib_dir)) =
                load_sibling_lib_rs(Path::new(&dir), config.vfs.as_ref(), config.allow_native_fs)
            {
                let mut lib_krate = parse(&lib_src, Limits::DEFAULT)
                    .map_err(|e| e.format_with_source(&lib_src, &lib_file))?;
                resolve_external_mods(
                    &mut lib_krate.items,
                    &lib_dir,
                    true,
                    config.vfs.as_ref(),
                    config.allow_native_fs,
                    0,
                )?;
                lib_crate_name = Some(crate_ident.clone());
                let lib_mod = Item {
                    attrs: Vec::new(),
                    vis: zipp_rsparse::ast::Visibility::Public,
                    kind: ItemKind::Mod(zipp_rsparse::ast::ModItem {
                        name: crate_ident,
                        items: Some(lib_krate.items),
                    }),
                    span: zipp_rsparse::token::Span::default(),
                };
                krate.items.insert(0, lib_mod);
            }
        }
    }

    // Resolve external `mod foo;` declarations from VFS or native disk
    resolve_external_mods(
        &mut krate.items,
        Path::new(&dir),
        true,
        config.vfs.as_ref(),
        config.allow_native_fs,
        0,
    )?;

    let seed = runtime_seed()?;
    let mut program = seed.program;
    let entry_fn = seed.entry_fn;
    let rt_slot = seed.rt_slot;
    let name_global = program.functions[entry_fn].name_global;

    // Collect crate symbols and allocate global slots on `program`
    let is_test_mode = config.mode == "test";
    let syms = CrateSymbols::collect(&krate.items, is_test_mode, lib_crate_name, &mut |gname| {
        let idx = program.global_names.len() as u32;
        program.global_names.push(gname);
        program.global_count = program.global_count.max(idx + 1);
        idx
    });

    let vfs_ref = config.vfs.as_ref();
    let allow_fs = config.allow_native_fs;
    let base_dir_path = PathBuf::from(&dir);
    let read_rel = |rel: &str| -> Result<String, String> {
        let norm = rel.replace('\\', "/").trim_start_matches("./").to_owned();
        if let Some(vfs) = vfs_ref {
            if let Some(bytes) = vfs.get(&norm) {
                return Ok(String::from_utf8_lossy(bytes).into_owned());
            }
        }
        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        if allow_fs {
            let p = base_dir_path.join(rel);
            return std::fs::read_to_string(&p)
                .map_err(|e| format!("include_str!({rel:?}): {e}"));
        }
        let _ = (&base_dir_path, allow_fs);
        Err(format!("include_str!({rel:?}): file not found"))
    };

    let file_ctx = FileCtx::new(&config.file, source, config.mode == "test", &read_rel);
    let prog_cell = RefCell::new(program);
    let mut emitter = RsEmitter::new(
        "__zipp_rs_entry",
        0,
        false,
        rt_slot,
        &prog_cell,
        &syms,
        &file_ctx,
        String::new(),
        None,
        std::collections::HashSet::new(),
    )?;
    emitter.emit_items(&krate.items)?;
    let ret = emitter.undefined()?;
    emitter.emit(Instr::Return { src: ret })?;
    let mut entry_proto = emitter.finish();
    entry_proto.name_global = name_global;

    let mut program = prog_cell.into_inner();
    program.functions.replace(entry_fn, entry_proto);
    program.rust_natives = true;
    // If running inside a Cargo package (`manifest_dir`), use `manifest_dir` as the
    // runtime filesystem base directory so relative paths like `"public/index.html"`
    // or `"data/store.json"` resolve relative to the project root (matching `cargo run`)!
    let runtime_base_dir = manifest
        .as_ref()
        .map(|m| m.manifest_dir.clone())
        .unwrap_or(dir);
    program.rust_ctx = Some(Box::new(RustCtx {
        file: config.file,
        base_dir: runtime_base_dir,
        argv: config.argv,
        vfs: config.vfs.map(Arc::new),
        allow_native_fs: config.allow_native_fs,
        allow_net: config.allow_net,
        stream_stdout: config.stream_stdout,
        mode: config.mode,
        pkg_env,
    }));
    Ok(program)
}

struct CargoManifest {
    name: String,
    version: String,
    description: String,
    manifest_dir: String,
}

fn discover_cargo_manifest(
    src_dir: &Path,
    vfs: Option<&HashMap<String, Vec<u8>>>,
    allow_native_fs: bool,
) -> Option<CargoManifest> {
    if let Some(vfs) = vfs {
        if let Some(bytes) = vfs.get("Cargo.toml") {
            let text = String::from_utf8_lossy(bytes);
            return parse_cargo_toml(&text, ".");
        }
    }
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    if allow_native_fs {
        for cand_dir in [Some(src_dir), src_dir.parent()] {
            if let Some(d) = cand_dir {
                let toml_path = d.join("Cargo.toml");
                if toml_path.is_file() {
                    if let Ok(text) = std::fs::read_to_string(&toml_path) {
                        let clean_dir = d.to_string_lossy().replace('\\', "/");
                        let clean_dir = clean_dir.strip_prefix("//?/").unwrap_or(&clean_dir).to_owned();
                        if let Some(m) = parse_cargo_toml(&text, &clean_dir) {
                            return Some(m);
                        }
                    }
                }
            }
        }
    }
    let _ = (src_dir, allow_native_fs);
    None
}

fn parse_cargo_toml(text: &str, manifest_dir: &str) -> Option<CargoManifest> {
    let mut in_pkg = false;
    let mut name = String::new();
    let mut version = String::from("0.1.0");
    let mut description = String::new();
    for raw_line in text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_pkg = line == "[package]";
            continue;
        }
        if !in_pkg {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            match k {
                "name" => name = v.to_owned(),
                "version" => version = v.to_owned(),
                "description" => description = v.to_owned(),
                _ => {}
            }
        }
    }
    if name.is_empty() {
        return None;
    }
    Some(CargoManifest {
        name,
        version,
        description,
        manifest_dir: manifest_dir.to_owned(),
    })
}

fn load_sibling_lib_rs(
    src_dir: &Path,
    vfs: Option<&HashMap<String, Vec<u8>>>,
    allow_native_fs: bool,
) -> Option<(String, String, PathBuf)> {
    if let Some(vfs) = vfs {
        for key in ["src/lib.rs", "lib.rs"] {
            if let Some(bytes) = vfs.get(key) {
                let d = Path::new(key).parent().unwrap_or(Path::new("")).to_path_buf();
                return Some((String::from_utf8_lossy(bytes).into_owned(), key.to_owned(), d));
            }
        }
    }
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    if allow_native_fs {
        let p = src_dir.join("lib.rs");
        if p.is_file() {
            if let Ok(src) = std::fs::read_to_string(&p) {
                return Some((src, p.to_string_lossy().into_owned(), src_dir.to_path_buf()));
            }
        }
    }
    let _ = (src_dir, allow_native_fs);
    None
}

fn extract_path_attr(attrs: &[zipp_rsparse::ast::Attribute]) -> Option<String> {
    for a in attrs {
        if a.path.last().map(|s| s.as_str()) == Some("path") {
            if let zipp_rsparse::ast::AttrArgs::Eq(expr) = &a.args {
                if let zipp_rsparse::ast::ExprKind::Literal(zipp_rsparse::ast::Literal::Str(s)) =
                    &expr.kind
                {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

fn resolve_external_mods(
    items: &mut [Item],
    cur_dir: &Path,
    is_dir_owner: bool,
    vfs: Option<&HashMap<String, Vec<u8>>>,
    allow_native_fs: bool,
    depth: usize,
) -> R<()> {
    if depth > 16 {
        return Err("Rust: module nesting depth exceeded".into());
    }
    for item in items.iter_mut() {
        let path_override = extract_path_attr(&item.attrs);
        if let ItemKind::Mod(m) = &mut item.kind {
            if let Some(sub_items) = &mut m.items {
                let sub_dir = cur_dir.join(&m.name);
                resolve_external_mods(sub_items, &sub_dir, true, vfs, allow_native_fs, depth + 1)?;
            } else {
                let (mod_src, mod_file, next_dir, child_is_dir_owner) = load_external_mod(
                    &m.name,
                    path_override.as_deref(),
                    cur_dir,
                    is_dir_owner,
                    vfs,
                    allow_native_fs,
                )?;
                let mut parsed = parse(&mod_src, Limits::DEFAULT)
                    .map_err(|e| e.format_with_source(&mod_src, &mod_file))?;
                resolve_external_mods(
                    &mut parsed.items,
                    &next_dir,
                    child_is_dir_owner,
                    vfs,
                    allow_native_fs,
                    depth + 1,
                )?;
                m.items = Some(parsed.items);
            }
        }
    }
    Ok(())
}

fn load_external_mod(
    mod_name: &str,
    path_override: Option<&str>,
    cur_dir: &Path,
    _is_dir_owner: bool,
    vfs: Option<&HashMap<String, Vec<u8>>>,
    allow_native_fs: bool,
) -> R<(String, String, PathBuf, bool)> {
    let candidates: Vec<(String, bool)> = if let Some(ov) = path_override {
        vec![(ov.to_owned(), ov.ends_with("mod.rs"))]
    } else {
        vec![
            (format!("{mod_name}.rs"), false),
            (format!("{mod_name}/mod.rs"), true),
        ]
    };

    if let Some(vfs) = vfs {
        let dir_prefix = cur_dir
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('.')
            .trim_matches('/')
            .to_owned();
        let make_key = |rel: &str| {
            if dir_prefix.is_empty() {
                rel.to_owned()
            } else {
                format!("{dir_prefix}/{rel}")
            }
        };
        for (cand, is_mod_rs) in &candidates {
            let k = make_key(cand);
            if let Some(bytes) = vfs.get(&k) {
                let next_dir = cur_dir.join(mod_name);
                return Ok((
                    String::from_utf8_lossy(bytes).into_owned(),
                    k,
                    next_dir,
                    *is_mod_rs,
                ));
            }
        }
    }

    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    if allow_native_fs {
        for (cand, is_mod_rs) in &candidates {
            let p = cur_dir.join(cand);
            if p.is_file() {
                let src = std::fs::read_to_string(&p)
                    .map_err(|e| format!("failed to read module {}: {e}", p.display()))?;
                let next_dir = cur_dir.join(mod_name);
                return Ok((src, p.to_string_lossy().into_owned(), next_dir, *is_mod_rs));
            }
        }
    }

    let _ = allow_native_fs;
    Err(format!(
        "Rust: cannot find file for `mod {mod_name};` in `{}`",
        cur_dir.display()
    ))
}

fn parent_dir_of(file: &str) -> String {
    Path::new(file)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ".".to_owned())
}
