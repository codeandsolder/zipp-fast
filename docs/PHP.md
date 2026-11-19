# PHP on ZIPP

ZIPP compiles PHP 8.x directly into its shared register bytecode and runs it on the same virtual machine as JavaScript, TypeScript, and Python — with no external PHP binary or PHP-to-JS transpilation.

## Running PHP

```sh
# Run a PHP script or template
zipp php app.php [args...]
zipp app.php [args...]
zipp run --lang=php app.php

# Inspect compiled ZIPP register bytecode
zipp php --bc app.php

# Built-in HTTP/1.1 SAPI server (shared-nothing per request + in-process OPcache)
zipp php -S 127.0.0.1:8000 -t examples/php/backend examples/php/backend/index.php

# Long-lived Worker Mode (boot once, handle requests on a warm VM)
zipp php --worker -S 127.0.0.1:8000 -t examples/php/backend examples/php/backend/worker.php
```

## Architecture

| Path | Role |
| --- | --- |
| [`crates/zipp-vm/src/lang/php/parser`](../crates/zipp-vm/src/lang/php/parser) | `zipp-phpparse`: pure-Rust (`#![forbid(unsafe_code)]`, zero-dependency) dual-mode HTML + PHP 8.x lexer and recursive-descent/Pratt parser. |
| [`crates/zipp-vm/src/lang/php/frontend`](../crates/zipp-vm/src/lang/php/frontend) | Namespace/`use` resolver, scope analyzer, ZIPP register bytecode emitter, pure-integer native JIT companion (`fast_int.rs`), and in-process OPcache for dynamically included files. |
| [`crates/zipp-vm/src/lang/php/runtime`](../crates/zipp-vm/src/lang/php/runtime) | PHP 8.x runtime (`__zipp_php`) written in JavaScript and compiled once into a memoized `RuntimeSeed`: 64-bit `BigInt` ints (auto-promoting to `float` on overflow), binary-safe byte strings, sticky-`shared` CoW arrays, `PhpRef` references, plain-object (`Object.create(null)`) OOP instances, full Reflection API, SPL data structures & iterators, lightweight `intl` shims, stdlib, and HTTP SAPI. |
| [`crates/zipp-vm/src/lang/php/vm`](../crates/zipp-vm/src/lang/php/vm) | Private VM natives (`__zipp_php_output`, `__zipp_php_include`, `__zipp_php_fs`, `__zipp_php_crypto`, `__zipp_php_env`, `__zipp_php_sapi`) bound by slot only in PHP programs. |
| [`crates/zipp-cli/src/php_server.rs`](../crates/zipp-cli/src/php_server.rs) | Multi-threaded native HTTP/1.1 SAPI server and Worker Mode runner. |

## What Runs

- **Dual-mode HTML templates:** inline HTML outside `<?php ... ?>`, `<?= ... ?>`, alternative control-structure syntax (`if (...): ... endif;`, `foreach (...): ... endforeach;`), heredoc/nowdoc, string interpolation (`"$var"`, `"$arr[key]"`, `"{$expr}"`), and nested output buffering (`ob_start`..`ob_get_clean`).
- **PHP 8.x value & array semantics:** exact 64-bit signed `int` (unboxed in `Value` for `±2^46`, overflow to `float`), binary-safe byte strings (`strlen("café") === 5`, `mb_strlen("café") === 4`), ordered map `array` with O(1) in-place sticky-`shared` copy-on-write (including nested CoW path writes like `$a['x']['y'][] = $v`) and zero-copy `foreach`, `&$x` references and call-site-invisible pass-by-reference (`sort($arr)`, `preg_match($p, $s, $m)`), PHP 8.x comparison rules (`==`, `===`, `<=>`, `??`, `?->`), and PHP assignment precedence (`if ($a || $b = foo())`).
- **Modern PHP 8.1–8.4 language features:** namespaces & group `use`, functions, closures (`use ($a, &$b)`, `Closure::bind`, `Closure::fromCallable`), arrow functions (`fn($x) => ...`), named arguments, variadic/spread arguments, `func_get_args`/`func_num_args`/`func_get_arg`, first-class callables (`foo(...)`), `match`, destructuring (`[$a, $b] = $arr`), `goto` & labels (with `try`/`finally` unwind), generators (`yield`, `yield $k => $v`, `yield from`), classes/interfaces/traits (`insteadof`/`as`), constructor property promotion, `readonly` classes & anonymous classes, PHP 8.3 typed class constants & dynamic class constant fetch (`Cls::{$name}`), PHP 8.4 parenthesis-less `new Foo()->bar()` chaining, asymmetric visibility (`public private(set)`), property hooks (`get` / `set`), Unit & Backed `enum`s, late static binding (`static::`), magic methods, runtime type checks (`declare(strict_types=1)`), attributes (`#[...]`) with full Reflection (`ReflectionClass`, `ReflectionMethod`, `ReflectionFunction`, `ReflectionParameter`, `ReflectionNamedType`/`UnionType`/`IntersectionType`, `ReflectionProperty`, `ReflectionAttribute`), `try`/`catch (A|B $e)`/`finally`, dynamic `include`/`require`/`include_once`/`require_once` with OPcache, and `spl_autoload_register` (Composer PSR-4 / static classmaps).
- **Frameworks & Backend stdlib:** verified byte-for-byte against PHP 8.4.15 with Composer 2 (`vendor/autoload.php`) across real **Laravel 11** (`illuminate/*` + `symfony/*`, 2,512 vendor files), **Slim 4** (`slim/slim` + `slim/psr7` + `nikic/fast-route`, 178 vendor files), and **CakePHP 5** (`cakephp/cakephp` + `cakephp/chronos` + `laminas/laminas-diactoros` + `league/container`, 1,048 vendor files) — both in CLI and live HTTP/1.1 Worker Mode (`zipp php --worker -S`). Includes `$_SERVER`, `$_GET`, `$_POST`, `$_COOKIE`, `$_FILES`, `$_REQUEST`, `$_ENV`, `$_SESSION`, `php://input`, `php://temp`, `header()`, `http_response_code()`, `setcookie()`, `session_*()`, JSON, PCRE (possessive quantifiers, `(?-1)` subpattern recursion, `(*SKIP)(*F)`, `(?|...)` branch reset groups, and Unicode `\pL`/`\pN` over ZIPP's `regress-fork`), `ctype_*`, `mb_*`, PHP 8.4 `array_find`/`array_any`/`array_all`, SPL (`ArrayObject`, `ArrayIterator`, `IteratorIterator`, `FilterIterator`, `CallbackFilterIterator`, `LimitIterator`, `AppendIterator`, `NoRewindIterator`, `CachingIterator`, `RecursiveArrayIterator`, `RecursiveIteratorIterator`, `SplObjectStorage`, `WeakMap`, `SplFileInfo`), lightweight `intl` (`Transliterator`, `Locale`, `MessageFormatter`), crypto (`hash`, `hash_hmac`, `md5`, `sha1`, `crc32`, OS-seeded CSPRNG `random_bytes`/`random_int`, `password_hash`/`password_verify`), DateTime (`DateTime`, `DateTimeImmutable`, `DateTimeZone`, `DateInterval`), streams/filesystem (`fopen`, `fstat`, `get_resource_type`, `stream_get_meta_data`, `stream_get_contents`), `filter_var`, and `zipp_handle_requests()` for Worker Mode.

## Performance

ZIPP lowers PHP hot paths directly onto ZIPP's existing register VM and native JIT without adding custom PHP VM opcodes:

- **Pure-int native JIT companion (`fast_int.rs`):** side-effect-free integer functions compile a guarded JS-`Number` companion (`±(2^30 - 1024)` for Tier-A native self-recursion, `±(2^52 - 1)` for Tier-C loop JIT) that transparently falls back to exact 64-bit `BigInt` on any overflow.
- **Fast OOP & arrays:** `Object.create(null)` instance properties (`obj.fp`) with inline bytecode `$this->prop` fast paths, monomorphic inline call sites for public direct methods, flattened zero-call `foreach` loops, inline `BigInt` arithmetic/comparisons, and direct bytecode intrinsics (`is_int`, `is_string`, `is_bool`, `is_float`, `is_null`, `is_array`, `strlen`, `count`).

On Windows x86-64 (`--release`, vs local PHP 8.4.15):
- Compute & OOP microbench (`sumLoop(100k)`, `fib(25)`, `arrayWork(10k)`, `oopWork(50k)`): **20.0 ms** total vs PHP 8.4.15 **52.6 ms** (**2.63× faster** overall: `sumLoop` 1.0 ms vs 2.5 ms, `fib(25)` 5.0 ms vs 39.3 ms, `oopWork` 8.0 ms vs 9.5 ms, `arrayWork` 5.0 ms vs 1.3 ms).
- Cold CLI framework boot + multi-route dispatch: Slim 4 **69.1 ms** (vs 60.0 ms), CakePHP 5 **117.7 ms** (vs 94.9 ms), Laravel 11 **111.6 ms** (vs 78.1 ms).
- Live HTTP/1.1 Worker Mode (`zipp php --worker -S`): Slim 4 **2.87 ms/req** (349 req/s), CakePHP 5 **3.45 ms/req** (290 req/s), Laravel 11 **5.95 ms/req** (168 req/s).

## Feature Flag & Sandbox Isolation

PHP support is behind the optional `php` Cargo feature on `zipp-vm`, `zipp-cli`, and `zipp-wasm` (excluded from `wasm-lite`). Under `safe-sandbox` and `wasm32`, native host filesystem and process environment access are compiled out and replaced by the caller-supplied virtual filesystem (`PhpProgramConfig::vfs`).

## Validation

```sh
cargo test -p zipp-phpparse
cargo test -p zipp-vm --features php --test php_language --test php_backend
cargo test -p zipp-cli --test php_http_server
```
