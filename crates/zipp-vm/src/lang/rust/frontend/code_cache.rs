//! Persistent disk bytecode cache for the Rust runtime seed (`crate::code_cache`).

use crate::bytecode::Program;
use crate::code_cache::{key, load, store, touch};

pub(super) use crate::code_cache::set_dir;

/// Load or compile (and persist) the Rust runtime `Program`.
pub(super) fn runtime(source: &str) -> Result<Program, String> {
    let Some((dir, k)) = key(&[b"rust-runtime-v2", source.as_bytes()]) else {
        let mut p = crate::compile_only(source, false)
            .map_err(|e| format!("Rust runtime compile error: {e}"))?;
        p.rust_natives = true;
        return Ok(p);
    };
    touch(dir);
    let name = format!("runtime-{k:016x}.bin");
    if let Some(program) =
        load(dir, &name, k).and_then(|p| crate::bytecode_codec::decode(&p, Some(source)).ok())
    {
        return Ok(program);
    }
    let mut program = crate::compile_only(source, false)
        .map_err(|e| format!("Rust runtime compile error: {e}"))?;
    program.rust_natives = true;
    if let Some(payload) = crate::bytecode_codec::encode(&program, Some(source)) {
        store(dir, &name, k, &payload);
    }
    Ok(program)
}
