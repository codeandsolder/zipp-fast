//! Private VM natives for the Rust frontend (`__zipp_rs_*`).
//!
//! Pure safe Rust (`#![forbid(unsafe_code)]`-compatible across native,
//! `safe-sandbox`, and `wasm32`). Bound by global slot ONLY when
//! `Program::rust_natives == true`.

use super::*;
use crate::value::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
use std::net::{TcpListener, TcpStream};

/// Per-VM state for Rust networking, monotonic clock epoch, and thread-safe env overrides.
pub(crate) struct RustVmState {
    pub env_overrides: HashMap<String, Option<String>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub epoch: std::time::Instant,
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    pub next_handle: u32,
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    pub listeners: HashMap<u32, TcpListener>,
    #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
    pub streams: HashMap<u32, TcpStream>,
    pub rng_state: u64,
}

impl Default for RustVmState {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        #[cfg(target_arch = "wasm32")]
        let seed = 0x9E37_79B9_7F4A_7C15u64;
        Self {
            env_overrides: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            epoch: std::time::Instant::now(),
            #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
            next_handle: 1,
            #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
            listeners: HashMap::new(),
            #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
            streams: HashMap::new(),
            rng_state: seed ^ 0xA076_1D64_78BD_642F,
        }
    }
}

impl RustVmState {
    fn next_rand_u64(&mut self) -> u64 {
        // SplitMix64
        self.rng_state = self.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl<'p> Vm<'p> {
    pub(crate) fn rust_native_call(&mut self, id: u16, args: &[Value]) -> Result<Value, Thrown> {
        let arg = |i: usize| args.get(i).copied().unwrap_or(Value::UNDEFINED);
        match id {
            native::RS_IO => self.rs_io_native(arg(0), arg(1)),
            native::RS_FS => self.rs_fs_native(arg(0), arg(1), arg(2), arg(3)),
            native::RS_ENV => self.rs_env_native(arg(0), arg(1), arg(2)),
            native::RS_TIME => self.rs_time_native(arg(0), arg(1)),
            native::RS_NET => self.rs_net_native(arg(0), arg(1), arg(2), arg(3)),
            native::RS_CRYPTO => self.rs_crypto_native(arg(0), arg(1), arg(2)),
            native::RS_PROC => self.rs_proc_native(arg(0), arg(1)),
            _ => Err(Thrown::from_static("InternalError: unknown Rust native")),
        }
    }

    fn ensure_rust_state(&mut self) -> &mut RustVmState {
        self.rust_state
            .get_or_insert_with(|| Box::new(RustVmState::default()))
    }

    fn rs_io_native(&mut self, op_v: Value, data_v: Value) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        match op.as_str() {
            "mode" => {
                let mode = self
                    .program
                    .rust_ctx
                    .as_deref()
                    .map(|c| c.mode.as_str())
                    .unwrap_or("run");
                return Ok(self.alloc_str(mode.to_owned()));
            }
            "drain_microtasks" => {
                self.microtask_checkpoint(false);
                return Ok(Value::UNDEFINED);
            }
            "stdout" => {
                let text = self.to_js_string(data_v)?;
                let stream = self
                    .program
                    .rust_ctx
                    .as_deref()
                    .is_some_and(|c| c.stream_stdout);
                #[cfg(not(target_arch = "wasm32"))]
                if stream {
                    use std::io::Write;
                    let _ = std::io::stdout().lock().write_all(text.as_bytes());
                    let _ = std::io::stdout().flush();
                    return Ok(Value::UNDEFINED);
                }
                let _ = stream;
                self.rust_output.extend_from_slice(text.as_bytes());
                return Ok(Value::UNDEFINED);
            }
            "stderr" => {
                let text = self.to_js_string(data_v)?;
                let stream = self
                    .program
                    .rust_ctx
                    .as_deref()
                    .is_some_and(|c| c.stream_stdout);
                #[cfg(not(target_arch = "wasm32"))]
                if stream {
                    use std::io::Write;
                    let _ = std::io::stderr().lock().write_all(text.as_bytes());
                    let _ = std::io::stderr().flush();
                    return Ok(Value::UNDEFINED);
                }
                let _ = stream;
                self.rust_stderr.extend_from_slice(text.as_bytes());
                return Ok(Value::UNDEFINED);
            }
            "stdin_line" => {
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                if self
                    .program
                    .rust_ctx
                    .as_deref()
                    .is_some_and(|c| c.allow_native_fs)
                {
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    return Ok(self.alloc_str(line));
                }
                return Ok(self.alloc_str(String::new()));
            }
            _ => Ok(Value::UNDEFINED),
        }
    }

    fn rs_fs_native(
        &mut self,
        op_v: Value,
        a_v: Value,
        b_v: Value,
        _c_v: Value,
    ) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let a = self.to_js_string(a_v)?;
        let b = self.to_js_string(b_v)?;
        let ctx = self.program.rust_ctx.as_deref();
        let allow_native = ctx.is_some_and(|c| c.allow_native_fs);

        // VFS lookup first (works in sandbox, WASM, and embedded tests).
        if let Some(vfs) = ctx.and_then(|c| c.vfs.as_ref()) {
            let norm = a.replace('\\', "/").trim_start_matches("./").to_owned();
            match op.as_str() {
                "read" => {
                    if let Some(bytes) = vfs.get(&norm) {
                        return Ok(self.alloc_str(String::from_utf8_lossy(bytes).into_owned()));
                    }
                    if !allow_native {
                        return Ok(Value::NULL);
                    }
                }
                "exists" | "is_file" => {
                    if vfs.contains_key(&norm) {
                        return Ok(Value::TRUE);
                    }
                    if !allow_native {
                        return Ok(Value::FALSE);
                    }
                }
                _ if !allow_native => return Ok(Value::NULL),
                _ => {}
            }
        }

        #[cfg(any(feature = "safe-sandbox", target_arch = "wasm32"))]
        {
            let _ = (op, a, b, allow_native);
            return Ok(Value::NULL);
        }

        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        {
            if !allow_native {
                return Ok(Value::NULL);
            }
            let base = ctx.map(|c| c.base_dir.as_str()).unwrap_or(".");
            let resolve = |p: &str| -> PathBuf {
                let path = Path::new(p);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    Path::new(base).join(path)
                }
            };
            let p = resolve(&a);
            match op.as_str() {
                "read" => match std::fs::read_to_string(&p) {
                    Ok(s) => Ok(self.alloc_str(s)),
                    Err(_) => Ok(Value::NULL),
                },
                "write" => Ok(Value::bool(std::fs::write(&p, b.as_bytes()).is_ok())),
                "append" => {
                    let ok = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&p)
                        .and_then(|mut f| f.write_all(b.as_bytes()))
                        .is_ok();
                    Ok(Value::bool(ok))
                }
                "exists" => Ok(Value::bool(p.exists())),
                "is_file" => Ok(Value::bool(p.is_file())),
                "is_dir" => Ok(Value::bool(p.is_dir())),
                "mkdir" => Ok(Value::bool(std::fs::create_dir(&p).is_ok())),
                "mkdir_all" => Ok(Value::bool(std::fs::create_dir_all(&p).is_ok())),
                "remove_file" => Ok(Value::bool(std::fs::remove_file(&p).is_ok())),
                "remove_dir" => Ok(Value::bool(std::fs::remove_dir(&p).is_ok())),
                "remove_dir_all" => Ok(Value::bool(std::fs::remove_dir_all(&p).is_ok())),
                "rename" => Ok(Value::bool(std::fs::rename(&p, resolve(&b)).is_ok())),
                "copy" => match std::fs::copy(&p, resolve(&b)) {
                    Ok(n) => Ok(Value::num(n as f64)),
                    Err(_) => Ok(Value::NULL),
                },
                "canonicalize" => match std::fs::canonicalize(&p) {
                    Ok(c) => {
                        let s = c.to_string_lossy().replace('\\', "/");
                        let s = s.strip_prefix("//?/").unwrap_or(&s).to_owned();
                        Ok(self.alloc_str(s))
                    }
                    Err(_) => Ok(Value::NULL),
                },
                "metadata" => match std::fs::metadata(&p) {
                    Ok(m) => {
                        let json = format!(
                            "{{\"len\":{},\"is_file\":{},\"is_dir\":{}}}",
                            m.len(),
                            m.is_file(),
                            m.is_dir()
                        );
                        Ok(self.alloc_str(json))
                    }
                    Err(_) => Ok(Value::NULL),
                },
                "read_dir" => match std::fs::read_dir(&p) {
                    Ok(rd) => {
                        let mut out = String::from("[");
                        let mut first = true;
                        for entry in rd.flatten() {
                            if !first {
                                out.push(',');
                            }
                            first = false;
                            let name = entry.file_name().to_string_lossy().into_owned();
                            let ep = entry.path().to_string_lossy().replace('\\', "/");
                            out.push_str("{\"name\":");
                            push_json_str(&mut out, &name);
                            out.push_str(",\"path\":");
                            push_json_str(&mut out, &ep);
                            out.push('}');
                        }
                        out.push(']');
                        Ok(self.alloc_str(out))
                    }
                    Err(_) => Ok(Value::NULL),
                },
                _ => Ok(Value::NULL),
            }
        }
    }

    fn rs_env_native(&mut self, op_v: Value, a_v: Value, b_v: Value) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let a = self.to_js_string(a_v)?;
        let b = self.to_js_string(b_v)?;
        let allow_native = self
            .program
            .rust_ctx
            .as_deref()
            .is_some_and(|c| c.allow_native_fs);
        match op.as_str() {
            "args" => {
                let mut out = String::from("[");
                if let Some(ctx) = self.program.rust_ctx.as_deref() {
                    for (i, arg) in ctx.argv.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        push_json_str(&mut out, arg);
                    }
                }
                out.push(']');
                Ok(self.alloc_str(out))
            }
            "var" => {
                if let Some(st) = self.rust_state.as_deref() {
                    if let Some(ov) = st.env_overrides.get(&a) {
                        return match ov {
                            Some(val) => Ok(self.alloc_str(val.clone())),
                            None => Ok(Value::NULL),
                        };
                    }
                }
                if let Some(ctx) = self.program.rust_ctx.as_deref() {
                    if let Some((_, val)) = ctx.pkg_env.iter().find(|(k, _)| k == &a) {
                        return Ok(self.alloc_str(val.clone()));
                    }
                }
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                if allow_native {
                    if let Ok(val) = std::env::var(&a) {
                        return Ok(self.alloc_str(val));
                    }
                }
                let _ = allow_native;
                Ok(Value::NULL)
            }
            "set_var" => {
                self.ensure_rust_state()
                    .env_overrides
                    .insert(a, Some(b));
                Ok(Value::UNDEFINED)
            }
            "remove_var" => {
                self.ensure_rust_state().env_overrides.insert(a, None);
                Ok(Value::UNDEFINED)
            }
            "vars" => {
                let mut map: HashMap<String, String> = HashMap::new();
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                if allow_native {
                    for (k, v) in std::env::vars() {
                        map.insert(k, v);
                    }
                }
                if let Some(st) = self.rust_state.as_deref() {
                    for (k, v) in &st.env_overrides {
                        match v {
                            Some(val) => {
                                map.insert(k.clone(), val.clone());
                            }
                            None => {
                                map.remove(k);
                            }
                        }
                    }
                }
                let mut out = String::from("[");
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('[');
                    push_json_str(&mut out, k);
                    out.push(',');
                    push_json_str(&mut out, v);
                    out.push(']');
                }
                out.push(']');
                Ok(self.alloc_str(out))
            }
            "current_dir" => {
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                if allow_native {
                    if let Ok(d) = std::env::current_dir() {
                        return Ok(self.alloc_str(d.to_string_lossy().replace('\\', "/")));
                    }
                }
                let base = self
                    .program
                    .rust_ctx
                    .as_deref()
                    .map(|c| c.base_dir.clone())
                    .unwrap_or_else(|| ".".to_owned());
                Ok(self.alloc_str(base))
            }
            "temp_dir" => {
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                if allow_native {
                    let d = std::env::temp_dir();
                    return Ok(self.alloc_str(d.to_string_lossy().replace('\\', "/")));
                }
                Ok(self.alloc_str("/tmp".to_owned()))
            }
            "pid" => {
                #[cfg(not(target_arch = "wasm32"))]
                return Ok(Value::num(std::process::id() as f64));
                #[cfg(target_arch = "wasm32")]
                return Ok(Value::int(1));
            }
            _ => Ok(Value::NULL),
        }
    }

    fn rs_time_native(&mut self, op_v: Value, arg_v: Value) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        match op.as_str() {
            "mono" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let ns = self.ensure_rust_state().epoch.elapsed().as_nanos() as i128;
                    return Ok(self.make_bigint(ns));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(self.make_bigint(0));
            }
            "unix" => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let ns = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as i128)
                        .unwrap_or(0);
                    return Ok(self.make_bigint(ns));
                }
                #[cfg(target_arch = "wasm32")]
                return Ok(self.make_bigint(0));
            }
            "sleep_ms" => {
                #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
                {
                    let raw = self.to_number_coerce(arg_v)?;
                    if raw.is_finite() && raw > 0.0 {
                        std::thread::sleep(std::time::Duration::from_millis(raw as u64));
                    }
                }
                let _ = arg_v;
                Ok(Value::UNDEFINED)
            }
            "rfc3339" => {
                let secs = self.to_number_coerce(arg_v)?.trunc() as i64;
                return Ok(self.alloc_str(format_unix_rfc3339(secs)));
            }
            _ => Ok(Value::UNDEFINED),
        }
    }

    fn rs_net_native(
        &mut self,
        op_v: Value,
        a_v: Value,
        b_v: Value,
        _c_v: Value,
    ) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let a = self.to_js_string(a_v)?;
        let b = self.to_js_string(b_v)?;
        #[cfg(any(feature = "safe-sandbox", target_arch = "wasm32"))]
        {
            let _ = (op, a, b);
            return Ok(Value::int(-1));
        }
        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        {
            if !self
                .program
                .rust_ctx
                .as_deref()
                .is_some_and(|c| c.allow_net)
            {
                return Ok(Value::int(-1));
            }
            match op.as_str() {
                "bind" => match TcpListener::bind(&a) {
                    Ok(listener) => {
                        let st = self.ensure_rust_state();
                        let id = st.next_handle;
                        st.next_handle += 1;
                        st.listeners.insert(id, listener);
                        Ok(Value::int(id as i32))
                    }
                    Err(_) => Ok(Value::int(-1)),
                },
                "local_addr" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let addr = self
                        .rust_state
                        .as_deref()
                        .and_then(|s| s.listeners.get(&id))
                        .and_then(|l| l.local_addr().ok())
                        .map(|a| a.to_string());
                    match addr {
                        Some(s) => Ok(self.alloc_str(s)),
                        None => Ok(Value::NULL),
                    }
                }
                "accept" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let accepted = self
                        .rust_state
                        .as_deref()
                        .and_then(|s| s.listeners.get(&id))
                        .and_then(|l| l.accept().ok());
                    match accepted {
                        Some((stream, peer)) => {
                            let st = self.ensure_rust_state();
                            let sid = st.next_handle;
                            st.next_handle += 1;
                            st.streams.insert(sid, stream);
                            Ok(self.alloc_str(format!("{sid}|{peer}")))
                        }
                        None => Ok(Value::NULL),
                    }
                }
                "connect" => match TcpStream::connect(&a) {
                    Ok(stream) => {
                        let st = self.ensure_rust_state();
                        let sid = st.next_handle;
                        st.next_handle += 1;
                        st.streams.insert(sid, stream);
                        Ok(Value::int(sid as i32))
                    }
                    Err(_) => Ok(Value::int(-1)),
                },
                "peer_addr" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let addr = self
                        .rust_state
                        .as_deref()
                        .and_then(|s| s.streams.get(&id))
                        .and_then(|l| l.peer_addr().ok())
                        .map(|a| a.to_string());
                    match addr {
                        Some(s) => Ok(self.alloc_str(s)),
                        None => Ok(Value::NULL),
                    }
                }
                "read_str" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let max_len: usize = b.parse().unwrap_or(4096).min(1024 * 1024);
                    let mut buf = vec![0u8; max_len];
                    let res = self
                        .rust_state
                        .as_deref_mut()
                        .and_then(|s| s.streams.get_mut(&id))
                        .map(|st| st.read(&mut buf));
                    match res {
                        Some(Ok(n)) => Ok(self.alloc_str(String::from_utf8_lossy(&buf[..n]).into_owned())),
                        _ => Ok(Value::NULL),
                    }
                }
                "read_all_str" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let mut buf = Vec::new();
                    let res = self
                        .rust_state
                        .as_deref_mut()
                        .and_then(|s| s.streams.get_mut(&id))
                        .map(|st| st.read_to_end(&mut buf));
                    match res {
                        Some(Ok(_)) => Ok(self.alloc_str(String::from_utf8_lossy(&buf).into_owned())),
                        _ => Ok(Value::NULL),
                    }
                }
                "read_http" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let parsed = self
                        .rust_state
                        .as_deref_mut()
                        .and_then(|s| s.streams.get_mut(&id))
                        .and_then(read_http1_request_json);
                    match parsed {
                        Some(json) => Ok(self.alloc_str(json)),
                        None => Ok(Value::NULL),
                    }
                }
                "write_str" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    let res = self
                        .rust_state
                        .as_deref_mut()
                        .and_then(|s| s.streams.get_mut(&id))
                        .map(|st| {
                            st.write_all(b.as_bytes()).and_then(|_| st.flush())
                        });
                    match res {
                        Some(Ok(())) => Ok(Value::num(b.len() as f64)),
                        _ => Ok(Value::int(-1)),
                    }
                }
                "close_listener" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    if let Some(st) = self.rust_state.as_deref_mut() {
                        st.listeners.remove(&id);
                    }
                    Ok(Value::UNDEFINED)
                }
                "close_stream" => {
                    let id: u32 = a.parse().unwrap_or(0);
                    if let Some(st) = self.rust_state.as_deref_mut() {
                        st.streams.remove(&id);
                    }
                    Ok(Value::UNDEFINED)
                }
                _ => Ok(Value::NULL),
            }
        }
    }

    fn rs_crypto_native(&mut self, op_v: Value, a_v: Value, b_v: Value) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let a = self.to_js_string(a_v)?;
        let b = self.to_js_string(b_v)?;
        let out = match op.as_str() {
            "sha256" => hex_encode(&sha256(a.as_bytes())),
            "sha512" => hex_encode(&sha512(a.as_bytes())),
            "hmac_sha256" => hex_encode(&hmac_sha256(a.as_bytes(), b.as_bytes())),
            "md5" => hex_encode(&md5(a.as_bytes())),
            "hex_encode" => hex_encode(a.as_bytes()),
            "hex_decode" => String::from_utf8_lossy(&hex_decode(&a)).into_owned(),
            "base64_encode" => base64_encode(a.as_bytes()),
            "base64_decode" => String::from_utf8_lossy(&base64_decode(&a)).into_owned(),
            "base64url_encode" => base64url_encode(a.as_bytes()),
            "base64url_decode" => String::from_utf8_lossy(&base64url_decode(&a)).into_owned(),
            "hash_password" => {
                let st = self.ensure_rust_state();
                let mut salt = [0u8; 16];
                salt[..8].copy_from_slice(&st.next_rand_u64().to_le_bytes());
                salt[8..].copy_from_slice(&st.next_rand_u64().to_le_bytes());
                let salt_hex = hex_encode(&salt);
                let dk = pbkdf_sha256(a.as_bytes(), &salt, 10_000);
                format!("$zsha256$10000${salt_hex}${}", hex_encode(&dk))
            }
            "verify_password" => {
                let ok = verify_password_hash(&a, &b);
                return Ok(Value::bool(ok));
            }
            "jwt_sign" => {
                // a = payload JSON string, b = secret
                let hdr = base64url_encode(br#"{"alg":"HS256","typ":"JWT"}"#);
                let pay = base64url_encode(a.as_bytes());
                let signing_input = format!("{hdr}.{pay}");
                let sig = base64url_encode(&hmac_sha256(b.as_bytes(), signing_input.as_bytes()));
                format!("{signing_input}.{sig}")
            }
            "jwt_verify" => {
                // a = token, b = secret -> returns payload JSON string or NULL
                let parts: Vec<&str> = a.split('.').collect();
                if parts.len() != 3 {
                    return Ok(Value::NULL);
                }
                let signing_input = format!("{}.{}", parts[0], parts[1]);
                let expected = base64url_encode(&hmac_sha256(b.as_bytes(), signing_input.as_bytes()));
                if !const_time_eq(expected.as_bytes(), parts[2].as_bytes()) {
                    return Ok(Value::NULL);
                }
                let payload = String::from_utf8_lossy(&base64url_decode(parts[1])).into_owned();
                payload
            }
            "random_bytes" => {
                let n: usize = a.parse().unwrap_or(16).min(65536);
                let st = self.ensure_rust_state();
                let mut bytes = Vec::with_capacity(n);
                while bytes.len() < n {
                    let r = st.next_rand_u64().to_le_bytes();
                    let take = (n - bytes.len()).min(8);
                    bytes.extend_from_slice(&r[..take]);
                }
                hex_encode(&bytes)
            }
            "uuid_v4" => {
                let st = self.ensure_rust_state();
                let hi = st.next_rand_u64();
                let lo = st.next_rand_u64();
                let mut bytes = [0u8; 16];
                bytes[..8].copy_from_slice(&hi.to_be_bytes());
                bytes[8..].copy_from_slice(&lo.to_be_bytes());
                bytes[6] = (bytes[6] & 0x0F) | 0x40;
                bytes[8] = (bytes[8] & 0x3F) | 0x80;
                let h = hex_encode(&bytes);
                format!(
                    "{}-{}-{}-{}-{}",
                    &h[0..8],
                    &h[8..12],
                    &h[12..16],
                    &h[16..20],
                    &h[20..32]
                )
            }
            _ => return Ok(Value::NULL),
        };
        Ok(self.alloc_str(out))
    }

    fn rs_proc_native(&mut self, op_v: Value, spec_v: Value) -> Result<Value, Thrown> {
        let op = self.to_js_string(op_v)?;
        let spec = self.to_js_string(spec_v)?;
        #[cfg(any(feature = "safe-sandbox", target_arch = "wasm32"))]
        {
            let _ = (op, spec);
            return Ok(Value::NULL);
        }
        #[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
        {
            if op != "run"
                || !self
                    .program
                    .rust_ctx
                    .as_deref()
                    .is_some_and(|c| c.allow_native_fs)
            {
                return Ok(Value::NULL);
            }
            // Minimal JSON parsing without external deps: extract program/args/cwd
            let Some((prog, args, cwd)) = parse_proc_spec(&spec) else {
                return Ok(Value::NULL);
            };
            let mut cmd = std::process::Command::new(&prog);
            cmd.args(&args);
            if !cwd.is_empty() {
                cmd.current_dir(&cwd);
            }
            match cmd.output() {
                Ok(out) => {
                    let code = out.status.code().unwrap_or(-1);
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let mut json = format!("{{\"code\":{code},\"stdout\":");
                    push_json_str(&mut json, &stdout);
                    json.push_str(",\"stderr\":");
                    push_json_str(&mut json, &stderr);
                    json.push('}');
                    Ok(self.alloc_str(json))
                }
                Err(e) => {
                    let mut json = String::from("{\"error\":");
                    push_json_str(&mut json, &e.to_string());
                    json.push('}');
                    Ok(self.alloc_str(json))
                }
            }
        }
    }
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn read_http1_request_json(stream: &mut TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut reader = BufReader::new(stream);
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).ok()? == 0 {
        return None;
    }
    let parts: Vec<&str> = req_line.trim_end().split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let method = parts[0];
    let uri = parts[1];
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        if headers.len() >= 256 {
            return None;
        }
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_owned();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0).min(16 * 1024 * 1024);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body);
    }
    let body_str = String::from_utf8_lossy(&body);
    let mut out = String::from("{\"method\":");
    push_json_str(&mut out, method);
    out.push_str(",\"uri\":");
    push_json_str(&mut out, uri);
    out.push_str(",\"headers\":{");
    for (i, (k, v)) in headers.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_str(&mut out, k);
        out.push(':');
        push_json_str(&mut out, v);
    }
    out.push_str("},\"body\":");
    push_json_str(&mut out, &body_str);
    out.push('}');
    Some(out)
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn parse_proc_spec(spec: &str) -> Option<(String, Vec<String>, String)> {
    // Simple extractor for `{"program":"...","args":[...],"cwd":"..."}`
    let prog = extract_json_string(spec, "\"program\":")?;
    let cwd = extract_json_string(spec, "\"cwd\":").unwrap_or_default();
    let mut args = Vec::new();
    if let Some(pos) = spec.find("\"args\":[") {
        let mut i = pos + 8;
        let bytes = spec.as_bytes();
        while i < bytes.len() && bytes[i] != b']' {
            if bytes[i] == b'"' {
                let (s, next) = read_json_str_at(spec, i)?;
                args.push(s);
                i = next;
            } else {
                i += 1;
            }
        }
    }
    Some((prog, args, cwd))
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn extract_json_string(s: &str, key: &str) -> Option<String> {
    let pos = s.find(key)? + key.len();
    let rest = s[pos..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let off = s.len() - rest.len();
    read_json_str_at(s, off).map(|(v, _)| v)
}

#[cfg(not(any(feature = "safe-sandbox", target_arch = "wasm32")))]
fn read_json_str_at(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            return Some((out, i + 1));
        }
        if b == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                other => out.push(other as char),
            }
            i += 1;
        } else {
            let ch = s[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    None
}

fn push_json_str(out: &mut String, s: &str) {
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

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xF) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let nib = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let (Some(h), Some(l)) = (nib(bytes[i]), nib(bytes[i + 1])) {
            out.push((h << 4) | l);
        }
        i += 2;
    }
    out
}

fn base64_encode(data: &[u8]) -> String {
    const TAB: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TAB[((n >> 18) & 63) as usize] as char);
        out.push(TAB[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TAB[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TAB[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u8;
    for &b in s.as_bytes() {
        if b == b'=' {
            break;
        }
        if let Some(v) = val(b) {
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buf >> bits) & 0xFF) as u8);
            }
        }
    }
    out
}

fn base64url_encode(data: &[u8]) -> String {
    base64_encode(data)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned()
}

fn base64url_decode(s: &str) -> Vec<u8> {
    let std = s.replace('-', "+").replace('_', "/");
    base64_decode(&std)
}

fn const_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn pbkdf_sha256(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut state = hmac_sha256(password, salt);
    for r in 0..rounds {
        let mut buf = [0u8; 36];
        buf[..32].copy_from_slice(&state);
        buf[32..].copy_from_slice(&r.to_le_bytes());
        state = hmac_sha256(password, &buf);
    }
    state
}

fn verify_password_hash(password: &str, encoded: &str) -> bool {
    let parts: Vec<&str> = encoded.split('$').collect();
    // ["", "zsha256", rounds, salt_hex, dk_hex]
    if parts.len() != 5 || parts[1] != "zsha256" {
        return false;
    }
    let Ok(rounds) = parts[2].parse::<u32>() else {
        return false;
    };
    if rounds == 0 || rounds > 200_000 {
        return false;
    }
    let salt = hex_decode(parts[3]);
    let expected = hex_decode(parts[4]);
    let actual = pbkdf_sha256(password.as_bytes(), &salt, rounds);
    const_time_eq(&actual, &expected)
}

/// Howard Hinnant's exact civil-from-days algorithm for UTC RFC-3339 formatting.
fn format_unix_rfc3339(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

// ---- Pure-Rust SHA-256, HMAC-SHA256, SHA-512, MD5 (zero dependencies) ----

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
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
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
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + data.len());
    let mut outer = Vec::with_capacity(64 + 32);
    for &b in &k {
        inner.push(b ^ 0x36);
        outer.push(b ^ 0x5c);
    }
    inner.extend_from_slice(data);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
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
    while msg.len() % 128 != 112 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(128) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            w[i] = u64::from_be_bytes(chunk[i * 8..i * 8 + 8].try_into().unwrap());
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
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut out = [0u8; 64];
    for (i, word) in h.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]).rotate_left(S[i]),
            );
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}
