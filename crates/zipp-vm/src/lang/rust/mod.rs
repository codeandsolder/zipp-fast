//! Rust language frontend: compiled straight to ZIPP's shared register bytecode.
//!
//! Its parts:
//! - `parser/`: pure-Rust (`#![forbid(unsafe_code)]`, zero-dependency) Rust lexer, AST, and parser (`zipp-rsparse`).
//! - `frontend/`: module loader, `use`/symbol resolver, `macro_rules!` & built-in macro expander,
//!   static numeric type inference, direct ZIPP register bytecode emitter, and persistent disk runtime cache.
//! - `runtime/`: Rust runtime (`__zipp_rs`), written in JavaScript and compiled once into a memoized `RuntimeSeed`.
//! - `vm/`: private VM natives (`__zipp_rs_io`, `__zipp_rs_fs`, `__zipp_rs_env`, `__zipp_rs_time`,
//!   `__zipp_rs_net`, `__zipp_rs_crypto`, `__zipp_rs_proc`) bound by slot ONLY in Rust programs.
//!
//! Everything except this table entry is behind the `rust` Cargo feature, so a build without Rust
//! still recognises `.rs` files and explains why it cannot run them.

use super::{Language, LanguageId};
use crate::embed::ScriptState;

#[cfg(feature = "rust")]
pub(crate) mod frontend;

#[cfg(feature = "rust")]
pub(super) use frontend::{compile, compile_program};

#[cfg(not(feature = "rust"))]
const NOT_BUILT: &str = "Rust support is not built; enable the `rust` Cargo feature";

/// The Rust entry of the language table.
pub(super) struct Rust;

impl Language for Rust {
    fn id(&self) -> LanguageId {
        LanguageId::Rust
    }

    fn names(&self) -> &'static [&'static str] {
        &["rust", "rs"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    /// `rust-script`, `cargo-script`, `rust`, `zipp-rust`.
    fn shebang(&self, command: &str) -> bool {
        matches!(command, "rust-script" | "cargo-script" | "rust" | "zipp-rust")
    }

    fn compile(&self, source: &str) -> Result<ScriptState, String> {
        #[cfg(feature = "rust")]
        {
            Ok(ScriptState::from_program(compile(source)?))
        }
        #[cfg(not(feature = "rust"))]
        {
            let _ = source;
            Err(NOT_BUILT.into())
        }
    }

    fn compile_to_text(&self, source: &str) -> Result<String, String> {
        #[cfg(feature = "rust")]
        {
            Ok(format!("{:#?}", compile(source)?))
        }
        #[cfg(not(feature = "rust"))]
        {
            let _ = source;
            Err(NOT_BUILT.into())
        }
    }
}
