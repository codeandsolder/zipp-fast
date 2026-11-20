<?php
declare(strict_types=1);

// Long-lived Worker Mode example — boots once, handles requests on a warm ZIPP VM!
// Run with:
//   zipp php --worker -S 127.0.0.1:8000 -t examples/php/backend examples/php/backend/worker.php

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

$bootTime = time();
$requestCount = 0;

$router = new Router();
$router->get('/', function () use (&$requestCount, $bootTime): void {
    header('Content-Type: application/json');
    echo json_encode([
        'mode' => 'worker',
        'sapi' => php_sapi_name(),
        'requests_served' => $requestCount,
        'uptime_sec' => time() - $bootTime,
    ]);
});
$router->get('/api/ping', function (): void {
    header('Content-Type: application/json');
    echo json_encode(['pong' => true]);
});

zipp_handle_requests(function (array $req) use ($router, &$requestCount): void {
    $requestCount++;
    $router->dispatch($req['method'], $req['path']);
});
