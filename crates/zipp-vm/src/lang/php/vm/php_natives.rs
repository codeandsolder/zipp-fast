//! Private VM natives for the PHP frontend (`__zipp_php_*`).
//!
//! Pure safe Rust (`#![forbid(unsafe_code)]`-compatible across native,
//! `safe-sandbox`, and `wasm32`). Bound by global slot ONLY when
//! `Program::php_natives == true`.

use super::*;
use crate::heap::{HeapObj, ObjMap, PropAttr};
use crate::value::Value;
use std::path::{Path, PathBuf};

/// Convert a PHP byte-string (Latin-1 code units U+0000..=U+00FF) to raw bytes.
fn js_bstr_to_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| (c as u32 & 0xFF) as u8).collect()
}

/// Convert raw bytes into a PHP byte-string (Latin-1 code units U+0000..=U+00FF).
fn bytes_to_js_bstr(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        out.push(char::from(b));
    }
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xF) as usize] as char);
    }
    out
}

impl<'p> Vm<'p> {
    pub(crate) fn php_native_call(&mut self, id: u16, args: &[Value]) -> Result<Value, Thrown> {
        let arg = |i: usize| args.get(i).copied().unwrap_or(Value::UNDEFINED);
        match id {
            native::PHP_OUTPUT => self.php_output_native(arg(0)),
            native::PHP_INCLUDE => self.php_include_native(arg(0), arg(1), arg(2)),
            native::PHP_FS => self.php_fs_native(arg(0), arg(1), arg(2), arg(3)),
            native::PHP_CRYPTO => self.php_crypto_native(arg(0), arg(1), arg(2), arg(3)),
            native::PHP_ENV => self.php_env_native(),
            native::PHP_SAPI => self.php_sapi_native(arg(0)),
            native::PHP_MAP_KV => self.php_map_kv_native(arg(0)),
            _ => Err(Thrown::from_static("InternalError: unknown PHP native")),
        }
    }

    fn php_output_native(&mut self, chunk: Value) -> Result<Value, Thrown> {
        let s = self.to_js_string(chunk)?;
        let bytes = js_bstr_to_bytes(&s);
        let stream_stdout = self
            .program
            .php_ctx
            .as_deref()
            .is_some_and(|c| c.stream_stdout);
        #[cfg(not(target_arch = "wasm32"))]
        if stream_stdout {
            use std::io::Write;
            let _ = std::io::stdout().lock().write_all(&bytes);
            return Ok(Value::UNDEFINED);
        }
        let _ = stream_stdout;
        self.php_output.extend_from_slice(&bytes);
        Ok(Value::UNDEFINED)
    }

    fn php_include_native(
        &mut self,
        path_val: Value,
        cur_dir_val: Value,
        mode_val: Value,
    ) -> Result<Value, Thrown> {
        let program: &'p crate::bytecode::Program = self.program;
        let Some(ctx) = program.php_ctx.as_deref() else {
            return Ok(Value::NULL);
        };
        let path_str = self.to_js_string(path_val)?;
        let cur_dir = self.to_js_string(cur_dir_val)?;
        let mode = self.to_number_coerce(mode_val)? as i32; // 0=include, 1=once, 2=eval
        let base_id = (self.main_func_count + self.eval_funcs.len()) as u32;

        let protos = if mode == 2 {
            (ctx.compile_unit)(None, "<eval>", &path_str, ctx.rt_slot, base_id, None)
                .map_err(|e| Thrown(format!("ParseError: {e}")))?
        } else {
            // Fast zero-syscall `include_once`/`require_once` check for absolute native paths!
            #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
            if mode == 1 && ctx.allow_native_fs && ctx.vfs.is_none() {
                let p = Path::new(&path_str);
                if p.is_absolute() {
                    let clean = normalize_fs_path(p);
                    let dedup = include_dedup_key(&clean);
                    if self.php_included.contains(&dedup) {
                        return Ok(Value::TRUE);
                    }
                }
            }
            match resolve_php_file_meta(ctx, &path_str, &cur_dir) {
                None => return Ok(Value::NULL),
                Some(ResolvedPhpFile::Vfs { key, disp, bytes }) => {
                    if mode == 1 && self.php_included.contains(&key) {
                        return Ok(Value::TRUE);
                    }
                    self.php_included.insert(key.clone());
                    let src = String::from_utf8_lossy(&bytes);
                    (ctx.compile_unit)(Some(&key), &disp, &src, ctx.rt_slot, base_id, None)
                        .map_err(|e| Thrown(format!("ParseError: {e}")))?
                }
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                Some(ResolvedPhpFile::Native {
                    dedup_key,
                    display_path,
                    disk_path,
                    mtime_nanos,
                    file_len,
                }) => {
                    if mode == 1 && self.php_included.contains(&dedup_key) {
                        return Ok(Value::TRUE);
                    }
                    self.php_included.insert(dedup_key.clone());
                    if let Some(hit) = (ctx.lookup_cached_by_stat)(
                        &dedup_key,
                        mtime_nanos,
                        file_len,
                        base_id,
                    ) {
                        hit
                    } else {
                        let Ok(bytes) = std::fs::read(&disk_path) else {
                            return Ok(Value::NULL);
                        };
                        let src = String::from_utf8_lossy(&bytes);
                        (ctx.compile_unit)(
                            Some(&dedup_key),
                            &display_path,
                            &src,
                            ctx.rt_slot,
                            base_id,
                            Some((mtime_nanos, file_len)),
                        )
                        .map_err(|e| Thrown(format!("ParseError: {e}")))?
                    }
                }
            }
        };

        if protos.is_empty() {
            return Ok(Value::NULL);
        }
        let count = protos.len() as u32;
        #[cfg(all(feature = "jit", target_arch = "x86_64"))]
        self.jit.php_functions_installed(base_id as usize, &protos);
        for f in protos {
            let node = self.eval_arena.funcs.push(self.eval_func_tail, f);
            self.eval_func_tail = Some(node);
            self.install_func(&node.value);
        }
        let top_id = base_id + count - 1;
        let fn_val = Value::heap(self.heap.alloc(HeapObj::Func(top_id)));
        self.realm_tag_new(fn_val.heap_index());
        Ok(fn_val)
    }

    /// Extract a JS `Map`'s live `(keys, vals)` directly into `[keysArr, valsArr, hasObj]`
    /// in Rust without a JS callback per entry (used by `R.iterStart` and JIT `foreach`).
    fn php_map_kv_native(&mut self, map_val: Value) -> Result<Value, Thrown> {
        if !map_val.is_heap() {
            let ks = self.alloc_array_current_realm(Vec::new());
            let vs = self.alloc_array_current_realm(Vec::new());
            return Ok(self.alloc_array_current_realm(vec![ks, vs, Value::FALSE]));
        }
        let idx = map_val.heap_index();
        let (out_keys, out_vals, has_obj) = match self.heap.get(idx) {
            HeapObj::Map { keys, vals } => {
                let len = keys.len().min(vals.len());
                let mut ok = Vec::with_capacity(len);
                let mut ov = Vec::with_capacity(len);
                let mut any_obj = false;
                for i in 0..len {
                    let k = keys[i];
                    if k.is_hole() {
                        continue;
                    }
                    let v = vals[i];
                    if !any_obj
                        && v.is_heap()
                        && matches!(self.heap.get(v.heap_index()), HeapObj::Object(_))
                    {
                        any_obj = true;
                    }
                    ok.push(k);
                    ov.push(v);
                }
                (ok, ov, any_obj)
            }
            _ => (Vec::new(), Vec::new(), false),
        };
        let ks = self.alloc_array_current_realm(out_keys);
        let vs = self.alloc_array_current_realm(out_vals);
        Ok(self.alloc_array_current_realm(vec![ks, vs, Value::bool(has_obj)]))
    }

    fn php_fs_native(
        &mut self,
        op_v: Value,
        a_v: Value,
        b_v: Value,
        _c_v: Value,
    ) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let a = self.to_js_string(a_v)?;
        let b = self.to_js_string(b_v)?;
        let ctx = self.program.php_ctx.as_deref();

        // Check VFS first (used in sandbox / WASM / embedded tests).
        if let Some(vfs) = ctx.and_then(|c| c.vfs.as_ref()) {
            let norm = a.replace('\\', "/").trim_start_matches("./").to_owned();
            match op.as_str() {
                "read" => {
                    if let Some(bytes) = vfs.get(&norm) {
                        return Ok(self.alloc_str(bytes_to_js_bstr(bytes)));
                    }
                    if !ctx.is_some_and(|c| c.allow_native_fs) {
                        return Ok(Value::NULL);
                    }
                }
                "exists" | "is_file" => {
                    if vfs.contains_key(&norm) {
                        return Ok(Value::TRUE);
                    }
                    if !ctx.is_some_and(|c| c.allow_native_fs) {
                        return Ok(Value::FALSE);
                    }
                }
                _ => {
                    if !ctx.is_some_and(|c| c.allow_native_fs) {
                        return Ok(Value::NULL);
                    }
                }
            }
        }

        #[cfg(any(feature = "safe-sandbox", target_arch = "wasm32"))]
        {
            let _ = (op, a, b);
            return Ok(Value::NULL);
        }

        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        {
            if !ctx.is_some_and(|c| c.allow_native_fs) {
                return Ok(Value::NULL);
            }
            let base = ctx.map(|c| c.base_dir.as_str()).unwrap_or(".");
            let resolve = |p: &str| -> PathBuf {
                let pb = Path::new(p);
                if pb.is_absolute() {
                    pb.to_path_buf()
                } else {
                    Path::new(base).join(pb)
                }
            };
            Ok(match op.as_str() {
                "read" => match std::fs::read(resolve(&a)) {
                    Ok(bytes) => self.alloc_str(bytes_to_js_bstr(&bytes)),
                    Err(_) => Value::NULL,
                },
                "write" => {
                    let bytes = js_bstr_to_bytes(&b);
                    Value::bool(std::fs::write(resolve(&a), &bytes).is_ok())
                }
                "append" => {
                    use std::io::Write;
                    let bytes = js_bstr_to_bytes(&b);
                    let ok = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(resolve(&a))
                        .and_then(|mut f| f.write_all(&bytes))
                        .is_ok();
                    Value::bool(ok)
                }
                "exists" => Value::bool(resolve(&a).exists()),
                "is_file" => Value::bool(resolve(&a).is_file()),
                "is_dir" => Value::bool(resolve(&a).is_dir()),
                "size" => match std::fs::metadata(resolve(&a)) {
                    Ok(m) => Value::num(m.len() as f64),
                    Err(_) => Value::NULL,
                },
                "mtime" => match std::fs::metadata(resolve(&a))
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                {
                    Some(d) => Value::num(d.as_secs() as f64),
                    None => Value::NULL,
                },
                "mkdir" => {
                    let p = resolve(&a);
                    let ok = if b == "1" {
                        std::fs::create_dir_all(p).is_ok()
                    } else {
                        std::fs::create_dir(p).is_ok()
                    };
                    Value::bool(ok)
                }
                "rmdir" => Value::bool(std::fs::remove_dir(resolve(&a)).is_ok()),
                "unlink" => Value::bool(std::fs::remove_file(resolve(&a)).is_ok()),
                "rename" => Value::bool(std::fs::rename(resolve(&a), resolve(&b)).is_ok()),
                "copy" => Value::bool(std::fs::copy(resolve(&a), resolve(&b)).is_ok()),
                "realpath" => match std::fs::canonicalize(resolve(&a)) {
                    Ok(p) => {
                        let s = p.to_string_lossy().replace('\\', "/");
                        let s = s.strip_prefix("//?/").unwrap_or(&s).to_owned();
                        self.alloc_str(s)
                    }
                    Err(_) => Value::NULL,
                },
                "cwd" => {
                    let s = std::env::current_dir()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|_| ".".to_owned());
                    self.alloc_str(s)
                }
                "tmpdir" => {
                    let s = std::env::temp_dir().to_string_lossy().replace('\\', "/");
                    self.alloc_str(s)
                }
                "scandir" => match std::fs::read_dir(resolve(&a)) {
                    Ok(rd) => {
                        let mut names: Vec<String> = vec![".".to_owned(), "..".to_owned()];
                        for ent in rd.flatten() {
                            if let Some(n) = ent.file_name().to_str() {
                                names.push(n.to_owned());
                            }
                        }
                        names.sort();
                        let vals: Vec<Value> = names.into_iter().map(|s| self.alloc_str(s)).collect();
                        self.alloc_array_current_realm(vals)
                    }
                    Err(_) => Value::NULL,
                },
                _ => Value::NULL,
            })
        }
    }

    fn php_crypto_native(
        &mut self,
        op_v: Value,
        a_v: Value,
        b_v: Value,
        c_v: Value,
    ) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        match op.as_str() {
            "hash" => {
                let algo = self.to_js_string(a_v)?;
                let data = js_bstr_to_bytes(&self.to_js_string(b_v)?);
                let binary = self.truthy(c_v);
                let digest = compute_hash(&algo, &data).ok_or_else(|| {
                    Thrown(format!("ValueError: hash(): Unknown hashing algorithm {algo}"))
                })?;
                let out = if binary {
                    bytes_to_js_bstr(&digest)
                } else {
                    hex_encode(&digest)
                };
                Ok(self.alloc_str(out))
            }
            "hmac" => {
                let algo = self.to_js_string(a_v)?;
                let data = js_bstr_to_bytes(&self.to_js_string(b_v)?);
                let key_and_flag = self.to_js_string(c_v)?;
                let (key_str, bin_flag) = key_and_flag
                    .rsplit_once('\0')
                    .unwrap_or((&key_and_flag, "0"));
                let key = js_bstr_to_bytes(key_str);
                let binary = bin_flag == "1";
                let digest = compute_hmac(&algo, &key, &data).ok_or_else(|| {
                    Thrown(format!("ValueError: hash_hmac(): Unknown hashing algorithm {algo}"))
                })?;
                let out = if binary {
                    bytes_to_js_bstr(&digest)
                } else {
                    hex_encode(&digest)
                };
                Ok(self.alloc_str(out))
            }
            "pbkdf2" => {
                let pw = js_bstr_to_bytes(&self.to_js_string(a_v)?);
                let salt = js_bstr_to_bytes(&self.to_js_string(b_v)?);
                let iters = (self.to_number_coerce(c_v)? as u32).clamp(1, 500_000);
                let dk = pbkdf2_hmac_sha256(&pw, &salt, iters, 32);
                Ok(self.alloc_str(hex_encode(&dk)))
            }
            "crc32" => {
                let data = js_bstr_to_bytes(&self.to_js_string(a_v)?);
                let c = crc32_ieee(&data);
                Ok(self.make_bigint(c as i128))
            }
            "random_bytes" => {
                let n = (self.to_number_coerce(a_v)? as usize).min(1 << 20);
                let bytes = csprng_bytes(n);
                Ok(self.alloc_str(bytes_to_js_bstr(&bytes)))
            }
            _ => Err(Thrown::from_static("Error: unknown crypto op")),
        }
    }

    fn php_env_native(&mut self) -> Result<Value, Thrown> {
        #[cfg_attr(any(feature = "safe-sandbox", target_arch = "wasm32"), allow(unused_mut))]
        let mut m = ObjMap::new();
        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        if self.program.php_ctx.as_deref().is_some_and(|c| c.allow_native_fs) {
            for (k, v) in std::env::vars_os() {
                if let (Some(k), Some(v)) = (k.to_str(), v.to_str()) {
                    let sv = self.alloc_str(v.to_owned());
                    m.define(k, sv, PropAttr::data());
                }
            }
        }
        Ok(self.alloc_object_current_realm(m))
    }

    fn php_sapi_native(&mut self, kind_v: Value) -> Result<Value, Thrown> {
        let kind = self.to_js_string(kind_v)?;
        let Some(ctx) = self.program.php_ctx.as_deref() else {
            return Ok(Value::NULL);
        };
        match kind.as_str() {
            "request" => match &ctx.sapi_request_json {
                Some(json) => Ok(self.alloc_str(json.clone())),
                None => Ok(Value::NULL),
            },
            "cli_info" => {
                let mut argv_json = String::from("[");
                for (i, a) in ctx.argv.iter().enumerate() {
                    if i > 0 {
                        argv_json.push(',');
                    }
                    push_json_escaped(&mut argv_json, a);
                }
                argv_json.push(']');
                let mut out = String::from("{\"script\":");
                push_json_escaped(&mut out, &ctx.file);
                out.push_str(",\"argv\":");
                out.push_str(&argv_json);
                out.push('}');
                Ok(self.alloc_str(out))
            }
            _ => Ok(Value::NULL),
        }
    }
}

fn push_json_escaped(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

enum ResolvedPhpFile {
    Vfs {
        key: String,
        disp: String,
        bytes: Vec<u8>,
    },
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    Native {
        dedup_key: String,
        display_path: String,
        disk_path: PathBuf,
        mtime_nanos: u128,
        file_len: u64,
    },
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn normalize_fs_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let stripped = raw.strip_prefix("//?/").unwrap_or(&raw);
    let mut prefix = String::new();
    let rest = if stripped.len() >= 2 && stripped.as_bytes()[1] == b':' {
        // Normalize Windows drive letter to uppercase for consistent keys.
        prefix.push((stripped.as_bytes()[0] as char).to_ascii_uppercase());
        prefix.push(':');
        &stripped[2..]
    } else if stripped.starts_with('/') {
        prefix.push('/');
        stripped
    } else {
        stripped
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if prefix.ends_with(':') {
        format!("{prefix}/{}", parts.join("/"))
    } else if prefix == "/" {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn include_dedup_key(clean_path: &str) -> String {
    if cfg!(windows) {
        clean_path.to_ascii_lowercase()
    } else {
        clean_path.to_owned()
    }
}

fn resolve_php_file_meta(
    ctx: &crate::bytecode::PhpCtx,
    req_path: &str,
    cur_dir: &str,
) -> Option<ResolvedPhpFile> {
    // 1. Check virtual filesystem first (for sandboxed / WASM / multi-file embedded tests).
    if let Some(vfs) = &ctx.vfs {
        let norm_req = req_path.replace('\\', "/");
        let candidates = [
            norm_req.trim_start_matches("./").to_owned(),
            if cur_dir.is_empty() || cur_dir == "." {
                norm_req.trim_start_matches("./").to_owned()
            } else {
                normalize_rel(&format!("{cur_dir}/{norm_req}"))
            },
            if ctx.base_dir.is_empty() || ctx.base_dir == "." {
                norm_req.trim_start_matches("./").to_owned()
            } else {
                normalize_rel(&format!("{}/{norm_req}", ctx.base_dir))
            },
        ];
        for cand in candidates {
            if let Some(bytes) = vfs.get(&cand) {
                return Some(ResolvedPhpFile::Vfs {
                    key: format!("vfs:{cand}"),
                    disp: cand,
                    bytes: bytes.clone(),
                });
            }
        }
    }

    // 2. Native filesystem (single `metadata()` syscall, zero `canonicalize()`, zero upfront read!).
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    if ctx.allow_native_fs {
        let p = Path::new(req_path);
        let candidates: Vec<PathBuf> = if p.is_absolute() {
            vec![p.to_path_buf()]
        } else {
            let mut v = Vec::with_capacity(3);
            if !cur_dir.is_empty() {
                v.push(Path::new(cur_dir).join(p));
            }
            if !ctx.base_dir.is_empty() {
                v.push(Path::new(&ctx.base_dir).join(p));
            }
            v.push(p.to_path_buf());
            v
        };
        for cand in candidates {
            if let Ok(meta) = std::fs::metadata(&cand) {
                if meta.is_file() {
                    let abs = if cand.is_absolute() {
                        cand
                    } else {
                        std::env::current_dir()
                            .map(|cwd| cwd.join(&cand))
                            .unwrap_or(cand)
                    };
                    let display_path = normalize_fs_path(&abs);
                    let dedup_key = include_dedup_key(&display_path);
                    let mtime_nanos = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    return Some(ResolvedPhpFile::Native {
                        dedup_key,
                        display_path,
                        disk_path: abs,
                        mtime_nanos,
                        file_len: meta.len(),
                    });
                }
            }
        }
    }

    None
}

fn normalize_rel(p: &str) -> String {
    let norm = p.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in norm.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

// ---- Pure-safe-Rust cryptographic primitives (SHA-256, SHA-512, SHA-1, MD5, HMAC, PBKDF2, CRC32, CSPRNG) ----

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha512(data: &[u8]) -> [u8; 64] {
    const K: [u64; 80] = [
        0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
        0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
        0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
        0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
        0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
        0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
        0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
        0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
        0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
        0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
        0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
        0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
        0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
        0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
        0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
        0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
        0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
        0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
    ];
    let mut h: [u64; 8] = [
        0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
        0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u128).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 128) != 112 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(128) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            w[i] = u64::from_be_bytes(chunk[8 * i..8 * i + 8].try_into().unwrap());
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 64];
    for (i, word) in h.iter().enumerate() {
        out[8 * i..8 * i + 8].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdcu32),
                _ => (b ^ c ^ d, 0xca62c1d6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d; d = c; c = b.rotate_left(30); b = a; a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7,12,17,22,7,12,17,22,7,12,17,22,7,12,17,22,
        5, 9,14,20,5, 9,14,20,5, 9,14,20,5, 9,14,20,
        4,11,16,23,4,11,16,23,4,11,16,23,4,11,16,23,
        6,10,15,21,6,10,15,21,6,10,15,21,6,10,15,21,
    ];
    const K: [u32; 64] = [
        0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,
        0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,
        0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,
        0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,
        0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,
        0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,
        0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,
        0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391,
    ];
    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes(chunk[4 * i..4 * i + 4].try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(S[i]));
            a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn compute_hash(algo: &str, data: &[u8]) -> Option<Vec<u8>> {
    match algo {
        "md5" => Some(md5(data).to_vec()),
        "sha1" => Some(sha1(data).to_vec()),
        "sha256" => Some(sha256(data).to_vec()),
        "sha512" => Some(sha512(data).to_vec()),
        _ => None,
    }
}

fn compute_hmac(algo: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let block = match algo {
        "md5" | "sha1" | "sha256" => 64,
        "sha512" => 128,
        _ => return None,
    };
    let mut k = if key.len() > block {
        compute_hash(algo, key)?
    } else {
        key.to_vec()
    };
    k.resize(block, 0);
    let mut ipad = vec![0x36u8; block];
    let mut opad = vec![0x5cu8; block];
    for i in 0..block {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Vec::with_capacity(block + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = compute_hash(algo, &inner)?;
    let mut outer = Vec::with_capacity(block + inner_hash.len());
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    compute_hash(algo, &outer)
}

fn pbkdf2_hmac_sha256(pw: &[u8], salt: &[u8], iters: u32, dk_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(dk_len);
    let mut block_idx: u32 = 1;
    while out.len() < dk_len {
        let mut s = Vec::with_capacity(salt.len() + 4);
        s.extend_from_slice(salt);
        s.extend_from_slice(&block_idx.to_be_bytes());
        let mut u = compute_hmac("sha256", pw, &s).unwrap();
        let mut t = u.clone();
        for _ in 1..iters {
            u = compute_hmac("sha256", pw, &u).unwrap();
            for i in 0..t.len() {
                t[i] ^= u[i];
            }
        }
        out.extend_from_slice(&t);
        block_idx += 1;
    }
    out.truncate(dk_len);
    out
}

/// Cryptographic random bytes seeded from OS entropy (zero `unsafe`, zero extra deps):
/// draws 256 bits from `std::collections::hash_map::RandomState` (which invokes
/// BCryptGenRandom on Windows and getrandom/urandom on Unix) + monotonic counter,
/// expanded via SHA-256 CTR.
fn csprng_bytes(len: usize) -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);

    let mut seed = [0u8; 40];
    for i in 0..4 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        seed[i * 8..(i + 1) * 8].copy_from_slice(&h.finish().to_le_bytes());
    }
    seed[32..40].copy_from_slice(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());

    let mut out = Vec::with_capacity(len);
    let mut ctr: u64 = 0;
    while out.len() < len {
        let mut block_in = [0u8; 48];
        block_in[..40].copy_from_slice(&seed);
        block_in[40..48].copy_from_slice(&ctr.to_le_bytes());
        let digest = sha256(&block_in);
        let take = (len - out.len()).min(32);
        out.extend_from_slice(&digest[..take]);
        ctr += 1;
    }
    out
}
