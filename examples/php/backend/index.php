<?php
declare(strict_types=1);

// Run with:
//   zipp php -S 127.0.0.1:8000 -t examples/php/backend examples/php/backend/index.php

spl_autoload_register(function (string $class): void {
    $prefix = 'App\\';
    if (str_starts_with($class, $prefix)) {
        $rel = str_replace('\\', '/', substr($class, strlen($prefix)));
        $file = __DIR__ . '/src/' . $rel . '.php';
        if (file_exists($file)) {
            require_once $file;
        }
    }
});

use App\Router;

function render(string $template, array $vars = []): string
{
    extract($vars);
    ob_start();
    include __DIR__ . '/views/' . $template;
    return ob_get_clean();
}

$router = new Router();

$router->get('/', function (): void {
    header('Content-Type: text/html; charset=UTF-8');
    echo render('home.phtml', [
        'title' => 'ZIPP PHP Backend',
        'items' => [
            ['id' => 1, 'name' => 'Shared JS/Python/PHP VM'],
            ['id' => 2, 'name' => 'In-process OPcache'],
            ['id' => 3, 'name' => 'Built-in HTTP SAPI & Worker Mode'],
        ],
    ]);
});

$router->get('/api/users/{id}', function (array $params): void {
    header('Content-Type: application/json');
    echo json_encode([
        'id' => (int)$params['id'],
        'role' => $_GET['role'] ?? 'member',
        'runtime' => PHP_VERSION,
    ]);
});

$router->post('/api/echo', function (): void {
    header('Content-Type: application/json');
    http_response_code(201);
    $body = json_decode(file_get_contents('php://input'), true);
    echo json_encode([
        'received' => $body,
        'hash' => hash('sha256', json_encode($body)),
    ]);
});

$method = $_SERVER['REQUEST_METHOD'] ?? 'GET';
$path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';
$router->dispatch($method, $path);
