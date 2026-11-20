//! Differential PHP 8.x language conformance tests.
//!
//! Each case runs through ZIPP's PHP frontend on the shared JS VM, verifies
//! exact expected stdout bytes, AND — when local `php` is available on PATH —
//! verifies byte-for-byte parity with PHP 8.4!
#![cfg(feature = "php")]

use std::process::Command;
use zipp_vm::frontend::{compile_php_program, PhpProgramConfig};

fn run_zipp_php(source: &str) -> String {
    let mut compiled = compile_php_program(
        source,
        PhpProgramConfig {
            file: "test.php".to_owned(),
            base_dir: ".".to_owned(),
            allow_native_fs: false,
            stream_stdout: false,
            ..Default::default()
        },
    )
    .expect("PHP source should compile");
    let res = compiled.state_mut().run_init();
    let bytes = compiled.state_mut().take_php_output();
    assert!(
        res.is_ok(),
        "unexpected PHP runtime error: {:?}\npartial output: {}\nsource:\n{source}",
        res.err(),
        String::from_utf8_lossy(&bytes)
    );
    String::from_utf8(bytes).expect("output should be valid UTF-8")
}

fn run_system_php(source: &str) -> Option<String> {
    let mut cmd = Command::new("php");
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "system php failed ({:?}):\nstderr: {}\nstdout: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );
    Some(String::from_utf8(out.stdout).expect("system php output UTF-8"))
}

fn system_php_version_id() -> Option<u32> {
    static VERSION: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        let out = Command::new("php")
            .args(["-r", "echo PHP_VERSION_ID;"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()?.trim().parse().ok()
    })
}

fn check_case(name: &str, source: &str, expected: &str) {
    check_case_min_php(name, source, expected, 80000);
}

fn check_case_min_php(name: &str, source: &str, expected: &str, min_php_version: u32) {
    let got = run_zipp_php(source);
    assert_eq!(
        got, expected,
        "[{name}] ZIPP PHP output mismatch\n--- got ---\n{got}\n--- expected ---\n{expected}"
    );
    if system_php_version_id().is_some_and(|v| v >= min_php_version) {
        if let Some(sys) = run_system_php(source) {
            assert_eq!(
                got, sys,
                "[{name}] ZIPP PHP diverged from system php\n--- zipp ---\n{got}\n--- php ---\n{sys}"
            );
        }
    }
}

#[test]
fn html_templates_and_echo() {
    check_case(
        "html_templates",
        r#"<h1>Hello</h1><?php $name = "ZIPP"; ?>
<p>Welcome <?= $name ?>!</p>
<?php if (true): ?>
<span>OK</span>
<?php endif; ?>"#,
        "<h1>Hello</h1><p>Welcome ZIPP!</p>\n<span>OK</span>\n",
    );
}

#[test]
fn scalars_64bit_int_and_php8_comparisons() {
    check_case(
        "scalars_and_comparisons",
        r#"<?php
$big = 9007199254740993; // beyond f64 2^53 safe integer!
echo $big, "\n";
echo ($big === 9007199254740992 ? "eq" : "ne"), "\n";
echo (is_int(4 / 2) ? "int" : "float"), " ", (is_float(5 / 2) ? "float" : "int"), "\n";
echo (1 === 1.0 ? "T" : "F"), " ", (1 == 1.0 ? "T" : "F"), "\n";
echo (0 == "foobar" ? "T" : "F"), " ", (42 == "42" ? "T" : "F"), "\n";
echo (bool)"0" ? "T" : "F", " ", (bool)[] ? "T" : "F", " ", (bool)"1" ? "T" : "F", "\n";
echo (1 <=> 2), " ", (2 <=> 2), " ", (3 <=> 2), "\n";
echo -2 ** 2, "\n";
echo "a" . (1 + 2) . "b\n";
"#,
        "9007199254740993\nne\nint float\nF T\nF T\nF F T\n-1 0 1\n-4\na3b\n",
    );
}

#[test]
fn cow_arrays_and_references() {
    check_case(
        "cow_arrays_and_refs",
        r#"<?php
$a = [10, 20, "x" => 30];
$b = $a;
$b[] = 40;
$b["x"] = 99;
echo implode(",", $a), "|", implode(",", $b), "\n";

function addItem(array $arr): array {
    $arr[] = 999;
    return $arr;
}
$c = addItem($a);
echo count($a), " ", count($c), "\n";

// Nested CoW path mutation
$nested = ["user" => ["tags" => ["a"]]];
$copy = $nested;
$copy["user"]["tags"][] = "b";
echo implode(",", $nested["user"]["tags"]), "|", implode(",", $copy["user"]["tags"]), "\n";

// Pass-by-reference & variable aliasing
function swap(int &$x, int &$y): void {
    $t = $x; $x = $y; $y = $t;
}
$p = 1; $q = 2;
swap($p, $q);
echo "$p,$q\n";

$items = [3, 1, 2];
sort($items);
echo implode("-", $items), "\n";

// Foreach by reference
$nums = [1, 2, 3];
foreach ($nums as &$n) {
    $n *= 10;
}
unset($n);
echo implode(",", $nums), "\n";
"#,
        "10,20,30|10,20,99,40\n3 4\na|a,b\n2,1\n1-2-3\n10,20,30\n",
    );
}

#[test]
fn closures_arrows_named_args_and_match() {
    check_case(
        "closures_arrows_match",
        r#"<?php
$factor = 3;
$mul = fn(int $x): int => $x * $factor;
echo $mul(7), "\n";

$count = 0;
$inc = function (int $step = 1) use (&$count): int {
    $count += $step;
    return $count;
};
echo $inc(), " ", $inc(step: 5), " ", $count, "\n";

function greet(string $who, string $prefix = "Hello", string $punct = "!"): string {
    return "$prefix, $who$punct";
}
echo greet(punct: "?", who: "PHP"), "\n";

foreach ([200, 201, 404, 500] as $code) {
    $label = match ($code) {
        200, 201 => "ok",
        404 => "missing",
        default => "err",
    };
    echo "$code:$label ";
}
echo "\n";

// Destructuring
[$first, , $third] = [10, 20, 30];
["id" => $id, "name" => $nm] = ["id" => 7, "name" => "Ada"];
echo "$first-$third-$id-$nm\n";
"#,
        "21\n1 6 6\nHello, PHP?\n200:ok 201:ok 404:missing 500:err \n10-30-7-Ada\n",
    );
}

#[test]
fn oop_traits_enums_lsb_magic_and_exceptions() {
    check_case(
        "oop_modern_php8",
        r#"<?php
namespace App\Demo;

interface Describable {
    public function describe(): string;
}

trait Timestamped {
    public function tag(): string {
        return "[" . static::kind() . "]";
    }
}

enum Role: string {
    case Admin = 'admin';
    case User = 'user';
    public function isPriv(): bool {
        return $this === self::Admin;
    }
}

class Entity implements Describable {
    use Timestamped;
    public static function kind(): string { return "entity"; }
    public function __construct(
        protected readonly string $name,
        private Role $role = Role::User,
    ) {}
    public function describe(): string {
        return $this->tag() . " " . $this->name . ":" . $this->role->value;
    }
}

class Account extends Entity {
    public static function kind(): string { return "account"; }
}

$acc = new Account("Alice", Role::from("admin"));
echo $acc->describe(), "\n";
echo ($acc instanceof Describable ? "yes" : "no"), " ", (Role::Admin->isPriv() ? "priv" : "std"), "\n";

// Try/catch/finally with union catch
try {
    throw new \InvalidArgumentException("bad input", 42);
} catch (\RuntimeException|\InvalidArgumentException $e) {
    echo "caught:" . $e->getMessage() . ":" . $e->getCode() . "\n";
} finally {
    echo "finally\n";
}
"#,
        "[account] Alice:admin\nyes priv\ncaught:bad input:42\nfinally\n",
    );
}

#[test]
fn generators_strings_json_pcre_and_output_buffering() {
    check_case(
        "gen_stdlib_ob",
        r#"<?php
function rangeGen(int $n): Generator {
    for ($i = 1; $i <= $n; $i++) {
        yield "k$i" => $i * $i;
    }
    return "done";
}
$g = rangeGen(3);
foreach ($g as $k => $v) {
    echo "$k=$v ";
}
echo $g->getReturn(), "\n";

// Binary-safe string length on UTF-8 + mb_strlen
$utf = "café";
echo strlen($utf), " ", mb_strlen($utf), "\n";

// JSON round-trip
$data = ["ok" => true, "nums" => [1, 2, 3], "name" => "zipp/php"];
$json = json_encode($data, JSON_UNESCAPED_SLASHES);
echo $json, "\n";
$dec = json_decode($json, true);
echo $dec["name"], ":", array_sum($dec["nums"]), "\n";

// PCRE
$m = [];
preg_match('/^\/users\/(?P<id>\d+)$/', '/users/42', $m);
echo "id=" . $m['id'] . "\n";
echo preg_replace('/\d+/', '#', 'a1b22c333'), "\n";

// Output buffering
ob_start();
echo "buffered-";
echo sprintf("%03d", 7);
$captured = ob_get_clean();
echo "OUT:$captured\n";
"#,
        "k1=1 k2=4 k3=9 done\n5 4\n{\"ok\":true,\"nums\":[1,2,3],\"name\":\"zipp/php\"}\nzipp/php:6\nid=42\na#b#c#\nOUT:buffered-007\n",
    );
}

#[test]
fn control_flow_break_levels_static_global_and_dynamic_scope() {
    check_case(
        "control_flow_and_scoping",
        r#"<?php
// Multi-level break 2 and continue 2
$out = [];
for ($i = 0; $i < 4; $i++) {
    for ($j = 0; $j < 4; $j++) {
        if ($j === 1) continue 2;
        if ($i === 2) break 2;
        $out[] = "$i:$j";
    }
}
echo implode(",", $out), "\n";

// Switch with fallthrough & default
foreach ([1, 2, 3, 9] as $x) {
    switch ($x) {
        case 1:
        case 2:
            echo "low ";
            break;
        case 3:
            echo "three ";
            break;
        default:
            echo "other ";
    }
}
echo "\n";

// Function static vars & global vars
$g = 100;
function counter(): int {
    static $n = 0;
    global $g;
    $n++;
    $g += $n;
    return $n;
}
echo counter(), ",", counter(), ",", counter(), "|g=$g\n";

// Dynamic scope: $$var, compact, extract
function dynScope(): string {
    $a = "alpha";
    $b = "beta";
    $name = "a";
    $arr = compact("a", "b");
    $extra = ["c" => "gamma"];
    extract($extra);
    return $$name . "-" . $arr["b"] . "-" . $c;
}
echo dynScope(), "\n";
"#,
        "0:0,1:0\nlow low three other \n1,2,3|g=106\nalpha-beta-gamma\n",
    );
}

#[test]
fn magic_methods_traits_conflicts_anon_class_and_reflection() {
    check_case(
        "magic_traits_reflection",
        r#"<?php
trait A {
    public function hi(): string { return "A"; }
}
trait B {
    public function hi(): string { return "B"; }
}

#[\Attribute]
class Route {
    public function __construct(public string $path) {}
}

#[Route('/items')]
class Bag {
    use A, B {
        B::hi insteadof A;
        A::hi as hiFromA;
    }
    private array $data = [];
    public function __set(string $k, mixed $v): void { $this->data[$k] = $v; }
    public function __get(string $k): mixed { return $this->data[$k] ?? null; }
    public function __call(string $m, array $args): string {
        return "$m(" . implode(",", $args) . ")";
    }
    public function __invoke(int $x): int { return $x + 1; }
    public function __toString(): string { return "Bag:" . count($this->data); }
}

$bag = new Bag();
$bag->foo = "bar";
echo $bag->foo, "|", $bag->hi(), "|", $bag->hiFromA(), "|", $bag->dyn(1, 2), "|", $bag(41), "|", (string)$bag, "\n";

// Anonymous class + first-class callable + nullsafe
$anon = new class(10) {
    public function __construct(public int $v) {}
    public function dbl(): int { return $this->v * 2; }
};
$fn = $anon->dbl(...);
$maybeNull = null;
echo $fn(), "|", ($maybeNull?->dbl() ?? "none"), "\n";

// Reflection + Attributes
$rc = new ReflectionClass(Bag::class);
$attrs = $rc->getAttributes(Route::class);
echo $rc->getShortName(), ":", $attrs[0]->newInstance()->path, "\n";
"#,
        "bar|B|A|dyn(1,2)|42|Bag:1\n20|none\nBag:/items\n",
    );
}

#[test]
fn stdlib_arrays_strings_urls_heredoc_and_strict_types() {
    check_case(
        "stdlib_deep",
        r#"<?php
declare(strict_types=1);

$nums = [5, 2, 8, 1, 9];
$evens = array_values(array_filter($nums, fn(int $n): bool => $n % 2 === 0));
$doubled = array_map(fn(int $n): int => $n * 2, $evens);
$sum = array_reduce($doubled, fn(int $acc, int $n): int => $acc + $n, 0);
echo implode(",", $doubled), "|sum=$sum\n";

$users = [
    ["id" => 1, "name" => "Ada", "score" => 90],
    ["id" => 2, "name" => "Linus", "score" => 95],
];
usort($users, fn(array $a, array $b): int => $b["score"] <=> $a["score"]);
echo implode(",", array_column($users, "name")), "\n";

// Heredoc + nowdoc + URL/base64/serialize
$who = "ZIPP";
$hd = <<<EOF
    Hello {$who}
    EOF;
$nd = <<<'EOF'
    Raw $who
    EOF;
echo $hd, "|", $nd, "\n";
echo base64_decode(base64_encode("hi\x00world")) === "hi\x00world" ? "binok" : "binfail", "\n";
echo http_build_query(["a" => "1 2", "b" => "x"]), "\n";
$u = parse_url("https://example.com:8080/api/v1?x=1#top");
echo $u["host"], ":", $u["port"], ":", $u["path"], "\n";
$s = serialize(["k" => [1, "two", true, null]]);
$u2 = unserialize($s);
echo $u2["k"][1], ":", count($u2["k"]), "\n";

// strict_types=1 enforces TypeError on string -> int
function takeInt(int $x): int { return $x; }
try {
    takeInt("123");
    echo "noerr\n";
} catch (TypeError $e) {
    echo "typeerr\n";
}
"#,
        "4,16|sum=20\nLinus,Ada\nHello ZIPP|Raw $who\nbinok\na=1+2&b=x\nexample.com:8080:/api/v1\ntwo:4\ntypeerr\n",
    );
}

#[test]
fn php83_84_syntax_hooks_asymmetric_vis_dynamic_const_new_chaining_and_goto() {
    check_case_min_php(
        "php83_84_modern_syntax",
        r#"<?php
class Box {
    public const int BASE = 10;
    public const int DOUBLED = self::BASE * 2;
    public static array $labels = [self::BASE => 'ten', self::DOUBLED => 'twenty'];

    public private(set) string $role = 'user';

    public string $fullName {
        get => $this->first . ' ' . $this->last;
        set(string $v) {
            [$this->first, $this->last] = explode(' ', $v, 2);
        }
    }

    public function __construct(
        private string $first = 'Ada',
        private string $last = 'Lovelace',
    ) {}

    public function promote(string $r): self {
        $this->role = $r;
        return $this;
    }
}

// PHP 8.3 dynamic class constant fetch + self:: static default
$k = 'DOUBLED';
echo Box::{$k}, '|', Box::$labels[20], "\n";

// PHP 8.4 parenthesis-less new chaining + property hooks + asymmetric visibility
$box = new Box()->promote('admin');
echo $box->role, '|', $box->fullName, "\n";
$box->fullName = 'Grace Hopper';
echo $box->fullName, "\n";
try {
    $box->role = 'hacker';
    echo "mutated\n";
} catch (Error $e) {
    echo "blocked\n";
}

// PHP 8.3 readonly anonymous class
$ro = new readonly class('locked') {
    public function __construct(public string $tag) {}
};
echo $ro->tag, "\n";

// Assignment precedence matching PHP 8.4 grammar
$a = true and false;
$x = false || $y = 42;
$r = 1 + $z = 5;
$m = !$q = 0;
echo json_encode([$a, $x, $y, $r, $z, $m, $q]), "\n";

// goto + labels (forward, loop, and try/finally unwind)
$log = [];
$n = 0;
loop_start:
$n++;
$log[] = "n$n";
if ($n < 3) goto loop_start;
try {
    $log[] = "try";
    goto after_try;
    $log[] = "dead";
} finally {
    $log[] = "fin";
}
after_try:
$log[] = "end";
echo implode(',', $log), "\n";
"#,
        "20|twenty\nadmin|Ada Lovelace\nGrace Hopper\nblocked\nlocked\n[true,true,42,6,5,true,0]\nn1,n2,n3,try,fin,end\n",
        80400,
    );
}

#[test]
fn reflection_di_container_closures_and_private_parent_dispatch() {
    check_case(
        "reflection_di_closures_parent_priv",
        r#"<?php
class Logger {
    public function tag(): string { return 'LOG'; }
}

class Service {
    public function __construct(
        private Logger $logger,
        private string $prefix = 'v1',
    ) {}
    public function info(string $msg): string {
        return $this->logger->tag() . ':' . $this->prefix . ':' . $msg;
    }
}

// Mini Laravel-style Reflection DI container
function make(string $cls): object {
    $rc = new ReflectionClass($cls);
    if (!$rc->isInstantiable()) throw new RuntimeException("Not instantiable: $cls");
    $ctor = $rc->getConstructor();
    if ($ctor === null) return $rc->newInstance();
    $args = [];
    foreach ($ctor->getParameters() as $p) {
        $ty = $p->getType();
        if ($ty instanceof ReflectionNamedType && !$ty->isBuiltin()) {
            $args[] = make($ty->getName());
        } elseif ($p->isDefaultValueAvailable()) {
            $args[] = $p->getDefaultValue();
        } else {
            throw new RuntimeException("Cannot resolve $" . $p->getName());
        }
    }
    return $rc->newInstanceArgs($args);
}

$svc = make(Service::class);
echo $svc->info('ok'), "\n";

// Closure::bind & Closure::fromCallable accessing private state (Composer/Macroable pattern)
class Vault {
    private string $secret = 'top-secret';
}
$v = new Vault();
$reader = Closure::bind(fn (): string => $this->secret, $v, Vault::class);
echo $reader(), "\n";
$fn = Closure::fromCallable([$svc, 'info']);
echo $fn('cb'), "\n";

// Parent private method invoked from parent method on subclass instance (Symfony Request pattern)
class BaseReq {
    private function checkInternal(): string { return 'base-priv'; }
    public function run(): string { return $this->checkInternal() . ':' . get_called_class(); }
}
class SubReq extends BaseReq {
    public function __call(string $m, array $a): string { return "magic:$m"; }
}
echo (new SubReq())->run(), "\n";

// func_get_args / func_num_args / func_get_arg
function variadicLegacy($first): string {
    return func_num_args() . ':' . func_get_arg(2) . ':' . implode('-', func_get_args());
}
echo variadicLegacy('a', 'b', 'c'), "\n";

// SPL ArrayObject & SplObjectStorage
$ao = new ArrayObject(['x' => 1, 'y' => 2]);
$ao['z'] = 3;
echo count($ao), ':', implode(',', iterator_to_array($ao)), "\n";
$store = new SplObjectStorage();
$store->attach($v, 'meta');
echo ($store->contains($v) ? 'has' : 'no'), ':', $store[$v], "\n";
"#,
        "LOG:v1:ok\ntop-secret\nLOG:v1:cb\nbase-priv:SubReq\n3:c:a-b-c\n3:1,2,3\nhas:meta\n",
    );
}

#[test]
fn framework_stdlib_pcre_possessive_unicode_ctype_mb_and_array84() {
    check_case_min_php(
        "framework_stdlib_and_pcre",
        r#"<?php
// Symfony RouteCompiler possessive quantifier `[^/]++` + `{...}sDu` delimiters
$m = [];
$ok = preg_match('{^/api/users/(?P<name>[^/]++)$}sDu', '/api/users/ada_lovelace', $m);
echo $ok, ':', $m['name'], "\n";

// preg_match_all initializes capturing group columns even on 0 matches (Laravel Route::compileParameterNames)
$m2 = [];
preg_match_all('/\{(.*?)\}/', '/static/path', $m2);
echo count($m2), ':', count($m2[1]), "\n";

// Laravel Str::slug Unicode property escapes `\pL` / `\pN`
$slug = strtolower(trim(preg_replace('/[^\-\pL\pN\s]+/u', '', 'Hello, Laravel 11!')));
$slug = preg_replace('/\s+/', '-', $slug);
echo $slug, "\n";

// PHP 8.4 array_* functions + ctype_* + mb_* + strtr + version_compare
$nums = [3, 7, 10, 14];
echo array_find($nums, fn ($n) => $n > 8), '|', array_find_key($nums, fn ($n) => $n > 8), "\n";
echo (array_any($nums, fn ($n) => $n % 2 === 0) ? 'Y' : 'N'),
     (array_all($nums, fn ($n) => $n > 0) ? 'Y' : 'N'),
     (array_is_list($nums) ? 'L' : 'M'), "\n";
echo strtr('hello {who}', ['{who}' => 'ZIPP']), '|',
     (ctype_alnum('abc123') ? '1' : '0'),
     (ctype_digit('42a') ? '1' : '0'), "\n";
echo implode('/', mb_split('[\s_]+', 'foo_bar baz')), '|',
     (version_compare('8.4.15', '8.4.1', '>=') ? 'ok' : 'no'), "\n";
"#,
        "1:ada_lovelace\n2:0\nhello-laravel-11\n10|2\nYYL\nhello ZIPP|10\nfoo/bar/baz|ok\n",
        80400,
    );
}

#[test]
fn slim_and_cakephp_framework_semantics() {
    check_case(
        "slim_and_cakephp_framework_semantics",
        r#"<?php
// 1. `new static::$prop()` + `$x instanceof static::$prop`
class TargetBox { public function hi(): string { return 'box-ok'; } }
abstract class BaseFactory {
    protected static string $cls;
    public static function make(): object {
        $o = new static::$cls();
        return ($o instanceof static::$cls) ? $o : throw new RuntimeException('bad');
    }
}
class BoxFactory extends BaseFactory { protected static string $cls = TargetBox::class; }
echo BoxFactory::make()->hi(), "\n";

// 2. Trait protected property owner rebinding + yield from in IteratorAggregate
trait HolderTrait {
    protected array $items = [];
    public function addItem(string $v): void { $this->items[] = $v; }
}
class Bag implements IteratorAggregate {
    use HolderTrait;
    public function getIterator(): Generator { yield from $this->items; }
}
$bag = new Bag();
$bag->addItem('a'); $bag->addItem('b');
echo implode(',', iterator_to_array($bag)), "\n";

// 3. Interface constant inheritance + callback builtin scope (`array_map([$this, 'protectedMethod'], ...)`)
interface StatusCodes { const FOUND = 1; }
class RouterBase implements StatusCodes {
    protected function wrap(string $s): string { return '[' . $s . ']'; }
    public function run(array $xs): string {
        return self::FOUND . ':' . implode(',', array_map([$this, 'wrap'], $xs));
    }
}
echo (new RouterBase())->run(['x', 'y']), "\n";

// 4. PCRE `(?-1)` recursion, `(*SKIP)(*F)`, `(?|...)` branch reset, trailing optional omission
$varRe = '\{\s*([a-zA-Z_][a-zA-Z0-9_-]*)\s*(?::\s*([^{}]*(?:\{(?-1)\}[^{}]*)*))?\}';
$segs = preg_split('~' . $varRe . '(*SKIP)(*F)|\[~x', '/u/{id:[0-9]{1,3}}[/opt');
echo count($segs), '|', $segs[0], '|', $segs[1], "\n";
preg_match_all('~' . $varRe . '~x', '/u/{name}/{id:[0-9]{1,3}}', $ms, PREG_SET_ORDER);
echo (isset($ms[0][2]) ? 'Y' : 'N'), '|', $ms[0][1], '|', $ms[1][1], ':', $ms[1][2], "\n";
preg_match('~^(?|/a/([^/]+)|/b/([0-9]+)())$~', '/a/foo', $m1);
preg_match('~^(?|/a/([^/]+)|/b/([0-9]+)())$~', '/b/42', $m2);
echo count($m1), ':', $m1[1], '|', count($m2), ':', $m2[1], "\n";

// 5. By-ref lvalue args on builtins (`array_unshift($r['pass'], ...)`, `sort($obj->list)`)
$r = ['pass' => ['two']];
array_unshift($r['pass'], 'one');
$s = new stdClass(); $s->list = [3, 1, 2];
sort($s->list);
echo implode('-', $r['pass']), '|', implode('-', $s->list), "\n";

// 6. First-class callable on array `[$obj, 'm'](...)` + ReflectionFunction + named arg unpack
class RuleProv {
    public function check(string $val, int $min, array $context = []): string {
        return $val . ':' . $min . ':' . ($context['tag'] ?? 'none');
    }
}
$cb = [new RuleProv(), 'check'](...);
$rparams = array_map(fn($p) => $p->getName(), (new ReflectionFunction($cb))->getParameters());
$callArgs = ['hello', 3, 'context' => ['tag' => 'ok']];
echo implode(',', $rparams), '|', $cb(...$callArgs), "\n";

// 7. SPL Iterators (`IteratorIterator`, `CallbackFilterIterator`, `LimitIterator`, `AppendIterator`)
$it1 = new ArrayIterator([1, 2, 3, 4, 5]);
$flt = new CallbackFilterIterator($it1, fn($v) => $v % 2 === 1);
$lim = new LimitIterator($flt, 1, 2);
$app = new AppendIterator();
$app->append($lim);
$app->append(new ArrayIterator([99]));
echo implode(',', array_values(iterator_to_array($app, false))), "\n";

// 8. Stream resources (`php://temp/maxmemory`, `fstat`, `get_resource_type`)
$fp = fopen('php://temp/maxmemory:1024', 'r+');
fwrite($fp, 'stream-data');
rewind($fp);
$st = fstat($fp);
echo get_resource_type($fp), '|', $st['size'], '|', stream_get_contents($fp), "\n";
fclose($fp);
"#,
        "box-ok\na,b\n1:[x],[y]\n2|/u/{id:[0-9]{1,3}}|/opt\nN|name|id:[0-9]{1,3}\n2:foo|3:42\none-two|1-2-3\nval,min,context|hello:3:ok\n3,5,99\nstream|11|stream-data\n",
    );
}

#[test]
fn jit_fast_int_overflow_and_fast_oop_parity() {
    check_case(
        "jit_fast_int_overflow_and_fast_oop_parity",
        r#"<?php
function sumTo(int $n): int {
    $t = 0;
    for ($i = 0; $i < $n; $i++) { $t += $i; }
    return $t;
}
function fibRec(int $n): int {
    if ($n < 2) return $n;
    return fibRec($n - 1) + fibRec($n - 2);
}
function doubleSteps(int $start, int $steps) {
    $x = $start;
    for ($i = 0; $i < $steps; $i++) { $x = $x + $x; }
    return $x;
}
function addTwo($a, $b) { return $a + $b; }

// Normal fast_int paths + coercion fallback + exact 64-bit/float overflow fallback!
echo sumTo(100), '|', fibRec(12), '|', addTwo("5", 7), "\n";
echo doubleSteps(1, 55), '|', gettype(doubleSteps(1, 55)), "\n";
$ov = addTwo(PHP_INT_MAX, 1);
echo gettype($ov), '|', ($ov > PHP_INT_MAX ? 'gt' : 'le'), "\n";
echo fibRec(-3), '|', sumTo(0), "\n";

// Fast `$this->prop`, monomorphic method calls, subclass override, private shadowing, and `&$this->prop` invalidation
class BaseCounter {
    public function __construct(public int $val = 0) {}
    public function inc(int $by): int { $this->val += $by; return $this->val; }
    public function get() { return $this->val; }
    private function secret(): string { return 'base-priv'; }
    public function callSecret(): string { return $this->secret(); }
}
class SubCounter extends BaseCounter {
    public function inc(int $by): int { $this->val += $by * 2; return $this->val; }
    public function secret(): string { return 'sub-pub'; }
}
$b = new BaseCounter(10);
$s = new SubCounter(10);
echo $b->inc(5), '|', $b->get(), '|', $s->inc(5), '|', $s->get(), "\n";
echo $b->callSecret(), '|', $s->callSecret(), '|', $s->secret(), "\n";
// Reference into fast property must invalidate direct `fp` shortcut cleanly!
$ref = &$b->val;
$ref = 100;
echo $b->inc(7), '|', $ref, "\n";

// Flattened `foreach` snapshot isolation + continue/break
$m = ['a' => 1, 'b' => 2, 'c' => 3, 'd' => 4];
$seen = [];
foreach ($m as $k => $v) {
    if ($k === 'b') continue;
    $m['z'] = 99;
    $seen[] = "$k=$v";
    if ($k === 'c') break;
}
echo implode(',', $seen), '|', count($m), "\n";
"#,
        "4950|144|12\n36028797018963968|integer\ndouble|le\n-3|0\n15|15|20|20\nbase-priv|base-priv|sub-pub\n107|107\na=1,c=3|5\n",
    );
}

#[test]
fn fast_jit_local_map_local_obj_and_lazy_builtins_parity() {
    check_case(
        "fast_jit_local_map_obj_lazy_builtins",
        r#"<?php
declare(strict_types=1);

// 1. LocalMap + ASCII string interpolation + strlen + count + foreach
function arrayWorkStr(int $n): int {
    $map = [];
    for ($i = 0; $i < $n; $i++) {
        $map["k$i"] = $i * 3 + 1;
    }
    $acc = count($map);
    foreach ($map as $k => $v) {
        $acc += $v + strlen($k);
    }
    return $acc;
}
function arrayWorkInt(int $n): int {
    $map = [];
    for ($i = 0; $i < $n; $i++) {
        $map[$i] = $i * 2;
    }
    $acc = 0;
    foreach ($map as $k => $v) {
        $acc += $k + $v;
    }
    return $acc + $map[3];
}
// Numeric string keys must coerce to int keys in PHP (declined by LocalMap non_int_key check!)
function numericStrKeys(int $n): string {
    $map = [];
    $map["0"] = $n;
    $map["01"] = $n + 1;
    $map["-5"] = $n + 2;
    $out = [];
    foreach ($map as $k => $v) {
        $out[] = gettype($k) . ':' . $k . '=' . $v;
    }
    return implode(',', $out);
}
// UTF-8 multi-byte strlen must keep exact byte length (declined by ASCII check!)
function utf8Len(int $n): int {
    $s = "café$n";
    return strlen($s);
}
// Overflow past 2^52 inside LocalMap value must bail cleanly to exact BigInt!
function arrayOverflow(int $base): int {
    $map = [];
    $map["k0"] = $base;
    $acc = 0;
    foreach ($map as $k => $v) {
        $acc += $v + $v;
    }
    return $acc;
}
echo arrayWorkStr(50), '|', arrayWorkInt(10), '|', utf8Len(7), "\n";
echo numericStrKeys(10), "\n";
$big = 1 << 52;
echo arrayOverflow($big), '|', gettype(arrayOverflow($big)), "\n";

// 2. LocalObj + fnum pure-int methods (promoted, regular ctor, void method, overflow bail, subclass!)
class FastCounter {
    public function __construct(public int $val = 0) {}
    public function inc(int $by): int {
        $this->val += $by;
        return $this->val;
    }
    public function addVoid(int $by): void {
        $this->val += $by;
    }
}
class SubFastCounter extends FastCounter {
    public function inc(int $by): int {
        $this->val += $by * 10;
        return $this->val;
    }
}
class Point2D {
    public int $x = 0;
    public int $y = 0;
    public function __construct(int $x, int $y = 5) {
        $this->x = $x;
        $this->y = $y;
    }
    public function shift(int $dx, int $dy): int {
        $this->x += $dx;
        $this->y += $dy;
        return $this->x + $this->y;
    }
}
function runCounter(int $n): int {
    $c = new FastCounter(0);
    for ($i = 0; $i < $n; $i++) {
        $c->inc($i);
    }
    $c->addVoid(5);
    return $c->val;
}
function runCounterBig(int $start, int $step): int {
    $c = new FastCounter($start);
    $c->inc($step);
    return $c->val;
}
function runSubCounter(int $n): int {
    $c = new SubFastCounter(1);
    for ($i = 0; $i < $n; $i++) {
        $c->inc($i);
    }
    return $c->val;
}
function runPoint(int $n): int {
    $p = new Point2D(2);
    $s = 0;
    for ($i = 0; $i < $n; $i++) {
        $s += $p->shift(1, 2);
    }
    return $s + $p->x * 100 + $p->y;
}
echo runCounter(100), '|', runCounterBig($big, $big), '|', runSubCounter(5), '|', runPoint(10), "\n";

// 3. Lazy builtin classes materialize cleanly on demand (Reflection, SPL, DateTime, Exceptions, Intl)
$rc = new ReflectionClass(FastCounter::class);
$it = new ArrayIterator(['x' => 1, 'y' => 2]);
$pairs = [];
foreach ($it as $k => $v) $pairs[] = "$k=$v";
$dt = new DateTimeImmutable('2026-01-02 03:04:05');
$di = new DateInterval('P2D');
$tz = new DateTimeZone('UTC');
$caught = 'none';
try {
    throw new BadMethodCallException('boom');
} catch (LogicException $e) {
    $caught = get_class($e) . ':' . $e->getMessage();
}
$loc = Locale::canonicalize('en-US');
$fmt = MessageFormatter::formatMessage('en_US', 'hi {name}', ['name' => 'zipp']);
$declared = get_declared_classes();
$hasRef = in_array('ReflectionClass', $declared, true) ? '1' : '0';
$hasSpl = in_array('RecursiveIteratorIterator', $declared, true) ? '1' : '0';
echo $rc->getShortName(), '|', implode(',', $pairs), '|', $dt->format('Y-m-d'), '|', $di->d, '|', $tz->getName(), '|', $caught, '|', $loc, '|', $fmt, '|', $hasRef, $hasSpl, "\n";
"#,
        "3915|141|6\ninteger:0=10,string:01=11,integer:-5=12\n9007199254740992|integer\n4955|9007199254740992|101|1460\nFastCounter|x=1,y=2|2026-01-02|2|UTC|BadMethodCallException:boom|en_US|hi zipp|11\n",
    );
}

#[test]
#[ignore]
fn compile_external_corpus_dir() {
    let dir = std::env::var("ZIPP_PHP_CORPUS_DIR").expect("set ZIPP_PHP_CORPUS_DIR");
    fn walk(p: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let ep = e.path();
                if ep.is_dir() {
                    walk(&ep, out);
                } else if ep.extension().is_some_and(|x| x == "php") {
                    out.push(ep);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(&dir), &mut files);
    let total = files.len();
    let mut ok = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let src: String = bytes.iter().map(|&b| b as char).collect();
        match compile_php_program(
            &src,
            PhpProgramConfig {
                file: f.display().to_string(),
                base_dir: ".".to_owned(),
                ..Default::default()
            },
        ) {
            Ok(_) => ok += 1,
            Err(e) => failures.push((f.display().to_string(), e)),
        }
    }
    println!("Compiled {ok}/{total} files ({} failures)", failures.len());
    for (f, e) in failures.iter().take(40) {
        println!("FAIL: {f}\n  {e}");
    }
    assert!(failures.is_empty(), "{} files failed to compile", failures.len());
}
