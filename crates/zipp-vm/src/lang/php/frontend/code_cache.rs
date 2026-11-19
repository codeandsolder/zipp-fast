//! Persistent disk bytecode cache for the PHP frontend (`crate::code_cache`):
//! 1. The compiled PHP runtime seed (`runtime-<key>.bin`) — eliminates re-parsing
//!    and re-compiling ~250 KB of runtime JS on every CLI startup.
//! 2. Per-entry OPcache packs (`module-<key>.bin`) — stores all dynamically
//!    `include`/`require`'d `.php` files for an entry script in a single binary
//!    pack, validated per file by `(mtime_nanos, file_len)` and `source_stamp`.
//!    Individual file blobs are decoded lazily on first `include`/`require` so
//!    startup pays zero upfront `FuncProto` decode or cloning cost.

use crate::bytecode::Program;
use crate::code_cache::{key, load, store, touch};
use std::sync::Arc;

pub(super) use crate::code_cache::set_dir;

/// Load or compile (and persist) the PHP runtime `Program`.
pub(super) fn runtime(source: &str) -> Result<Program, String> {
    let Some((dir, k)) = key(&[b"php-runtime", source.as_bytes()]) else {
        let mut p = crate::compile_only(source, false)
            .map_err(|e| format!("PHP runtime compile error: {e}"))?;
        p.php_natives = true;
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
        .map_err(|e| format!("PHP runtime compile error: {e}"))?;
    program.php_natives = true;
    if let Some(payload) = crate::bytecode_codec::encode(&program, Some(source)) {
        store(dir, &name, k, &payload);
    }
    Ok(program)
}

const PACK_MAGIC: &[u8; 4] = b"ZPO2";
const MAX_PACK_ENTRIES: usize = 4096;

pub(super) struct PackEntry {
    pub file_key: String,
    pub mtime_nanos: u128,
    pub file_len: u64,
    pub stamp: u64,
    pub blob: Arc<[u8]>,
}

/// Load the single-file OPcache pack previously persisted for `entry_file`
/// without decoding individual `FuncProto` payloads yet.
pub(super) fn load_opcache_pack(entry_file: &str) -> Vec<PackEntry> {
    let Some((dir, k)) = key(&[b"php-opcache-pack-v2", entry_file.as_bytes()]) else {
        return Vec::new();
    };
    let Some(bytes) = load(dir, &format!("module-{k:016x}.bin"), k) else {
        return Vec::new();
    };
    decode_pack(&bytes).unwrap_or_default()
}

/// Persist the single-file OPcache pack for `entry_file`.
pub(super) fn store_opcache_pack(entry_file: &str, entries: &[PackEntry]) {
    if entries.is_empty() {
        return;
    }
    let Some((dir, k)) = key(&[b"php-opcache-pack-v2", entry_file.as_bytes()]) else {
        return;
    };
    let total_blob: usize = entries.iter().map(|e| e.blob.len() + e.file_key.len() + 40).sum();
    let mut buf = Vec::with_capacity(8 + total_blob);
    buf.extend_from_slice(PACK_MAGIC);
    let count = entries.len().min(MAX_PACK_ENTRIES);
    buf.extend_from_slice(&(count as u32).to_le_bytes());
    for entry in entries.iter().take(count) {
        let kb = entry.file_key.as_bytes();
        buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
        buf.extend_from_slice(kb);
        buf.extend_from_slice(&entry.mtime_nanos.to_le_bytes());
        buf.extend_from_slice(&entry.file_len.to_le_bytes());
        buf.extend_from_slice(&entry.stamp.to_le_bytes());
        buf.extend_from_slice(&(entry.blob.len() as u32).to_le_bytes());
        buf.extend_from_slice(&entry.blob);
    }
    store(dir, &format!("module-{k:016x}.bin"), k, &buf);
}

fn decode_pack(bytes: &[u8]) -> Option<Vec<PackEntry>> {
    if bytes.len() < 8 || &bytes[..4] != PACK_MAGIC {
        return None;
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
    if count > MAX_PACK_ENTRIES {
        return None;
    }
    let mut pos = 8usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let klen = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos += 4;
        let kbytes = bytes.get(pos..pos + klen)?;
        pos += klen;
        let file_key = std::str::from_utf8(kbytes).ok()?.to_owned();
        let mtime_nanos = u128::from_le_bytes(bytes.get(pos..pos + 16)?.try_into().ok()?);
        pos += 16;
        let file_len = u64::from_le_bytes(bytes.get(pos..pos + 8)?.try_into().ok()?);
        pos += 8;
        let stamp = u64::from_le_bytes(bytes.get(pos..pos + 8)?.try_into().ok()?);
        pos += 8;
        let blen = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?) as usize;
        pos += 4;
        let blob: Arc<[u8]> = bytes.get(pos..pos + blen)?.into();
        pos += blen;
        out.push(PackEntry {
            file_key,
            mtime_nanos,
            file_len,
            stamp,
            blob,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode_codec::{decode, decode_functions, encode, encode_functions};

    #[test]
    fn php_runtime_and_opcache_pack_round_trip() {
        let mut program = crate::compile_only(super::super::RUNTIME_JS, false).unwrap();
        program.php_natives = true;
        let bytes = encode(&program, Some(super::super::RUNTIME_JS)).unwrap();
        let back = decode(&bytes, Some(super::super::RUNTIME_JS)).unwrap();
        assert_eq!(back.php_natives, true);
        assert_eq!(back.functions.len(), program.functions.len());

        let protos = super::super::compile_dynamic_unit_cached(
            None,
            "pack_test.php",
            "<?php function f(int $x): int { return $x + 1; }",
            0,
            0,
            None,
        )
        .unwrap();
        let blob: Arc<[u8]> = encode_functions(&protos).into();
        let mut raw = Vec::new();
        raw.extend_from_slice(PACK_MAGIC);
        raw.extend_from_slice(&1u32.to_le_bytes());
        let key = b"pack_test.php";
        raw.extend_from_slice(&(key.len() as u32).to_le_bytes());
        raw.extend_from_slice(key);
        raw.extend_from_slice(&123456789u128.to_le_bytes());
        raw.extend_from_slice(&42u64.to_le_bytes());
        raw.extend_from_slice(&9999u64.to_le_bytes());
        raw.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        raw.extend_from_slice(&blob);

        let decoded = decode_pack(&raw).expect("pack decode");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].file_key, "pack_test.php");
        assert_eq!(decoded[0].mtime_nanos, 123456789);
        assert_eq!(decoded[0].file_len, 42);
        assert_eq!(decoded[0].stamp, 9999);
        let restored = decode_functions(&decoded[0].blob).unwrap();
        assert_eq!(format!("{restored:?}"), format!("{protos:?}"));
    }
}
