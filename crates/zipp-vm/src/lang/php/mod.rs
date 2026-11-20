//! PHP 8.x language frontend: compiled straight to ZIPP's shared register bytecode.
//!
//! Its parts:
//! - `parser/`: dual-mode HTML/PHP lexer, AST, and parser (`zipp-phpparse`).
//! - `frontend/`: namespace resolver, scope analyzer, bytecode emitter, OPcache,
//!   and SAPI request/response helpers.
//! - `runtime/`: the PHP 8.x runtime (`__zipp_php`), written in JavaScript and
//!   compiled once into a memoized `RuntimeSeed`.
//! - `vm/`: private VM natives (`__zipp_php_output`, `__zipp_php_include`,
//!   `__zipp_php_fs`, `__zipp_php_crypto`, `__zipp_php_env`, `__zipp_php_sapi`).
//!
//! All of it is behind the `php` Cargo feature except this table entry, so a
//! build without PHP still recognises `.php` files and says why it cannot run them.

use super::{Language, LanguageId};
use crate::embed::ScriptState;

#[cfg(feature = "php")]
pub(crate) mod frontend;

#[cfg(feature = "php")]
pub(super) use frontend::{compile, compile_program};

#[cfg(not(feature = "php"))]
const NOT_BUILT: &str = "PHP support is not built; enable the `php` Cargo feature";

/// The PHP entry of the language table.
pub(super) struct Php;

impl Language for Php {
    fn id(&self) -> LanguageId {
        LanguageId::Php
    }

    fn names(&self) -> &'static [&'static str] {
        &["php"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["php", "phtml"]
    }

    /// `php`, `php8`, `php8.4`, etc.
    fn shebang(&self, command: &str) -> bool {
        command
            .strip_prefix("php")
            .is_some_and(|s| s.is_empty() || s.chars().all(|c| c.is_ascii_digit() || c == '.'))
    }

    fn compile(&self, source: &str) -> Result<ScriptState, String> {
        #[cfg(feature = "php")]
        {
            Ok(ScriptState::from_program(compile(source)?))
        }
        #[cfg(not(feature = "php"))]
        {
            let _ = source;
            Err(NOT_BUILT.into())
        }
    }

    fn compile_to_text(&self, source: &str) -> Result<String, String> {
        #[cfg(feature = "php")]
        {
            Ok(format!("{:#?}", compile(source)?))
        }
        #[cfg(not(feature = "php"))]
        {
            let _ = source;
            Err(NOT_BUILT.into())
        }
    }
}
