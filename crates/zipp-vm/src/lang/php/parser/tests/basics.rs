use zipp_phpparse::{parse, Limits};

#[test]
fn parses_mixed_html_and_php() {
    let src = r#"<h1><?= $title ?></h1>
<?php
declare(strict_types=1);
namespace App\Http;
use App\Models\{User, Post as Article};

#[Route('/users', methods: ['GET'])]
readonly class UserController extends Base implements JsonSerializable {
    use Loggable;
    public function __construct(private string $prefix = "hi") {}

    public function index(int|string $id, ?array $opts = null): string {
        $msg = "Hello $id, {$this->prefix}!";
        $res = match ($id) {
            1, 2 => "small",
            default => "other",
        };
        return $msg . $res;
    }
}
?>
<p>Done</p>"#;
    let prog = parse(src, Limits::DEFAULT).expect("should parse modern PHP 8.4");
    assert!(prog.stmts.len() >= 5);
}

#[test]
fn parses_alternative_control_structures_and_heredoc() {
    let src = r#"<?php
$name = 'World';
$txt = <<<EOT
    Hello $name!
    Line 2
    EOT;
if ($name === 'World'): ?>
  <span><?= $txt ?></span>
<?php else: ?>
  <span>Other</span>
<?php endif;
"#;
    let prog = parse(src, Limits::DEFAULT).expect("should parse template syntax + heredoc");
    assert!(!prog.stmts.is_empty());
}

#[test]
fn parses_enum_closures_and_generators() {
    let src = r#"<?php
enum Status: string {
    case Ok = 'ok';
    case Err = 'err';
    public function label(): string { return $this->value; }
}
$fn = fn(int $x): int => $x * 2;
$cb = function (int &$a) use ($fn): int { return $fn($a++); };
function gen(): Generator {
    yield 'a' => 1;
    yield from [2, 3];
}
"#;
    let prog = parse(src, Limits::DEFAULT).expect("should parse enum, closures, generators");
    assert!(prog.stmts.len() >= 4);
}

#[test]
#[ignore]
fn parse_external_corpus_dir() {
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
        match parse(&src, Limits::DEFAULT) {
            Ok(_) => ok += 1,
            Err(e) => failures.push((f.display().to_string(), e.to_string())),
        }
    }
    println!("Parsed {ok}/{total} files ({} failures)", failures.len());
    for (f, e) in failures.iter().take(40) {
        println!("FAIL: {f}\n  {e}");
    }
    assert!(failures.is_empty(), "{} files failed to parse", failures.len());
}
