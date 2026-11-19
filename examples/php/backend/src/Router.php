<?php
declare(strict_types=1);

namespace App;

/**
 * Minimal PSR-4 style router demonstrating PHP 8.x OOP, closures, and PCRE on ZIPP.
 */
final class Router
{
    /** @var array<int, array{method: string, regex: string, handler: callable}> */
    private array $routes = [];

    public function add(string $method, string $pattern, callable $handler): self
    {
        $regex = '#^' . preg_replace('#\{([a-zA-Z_][a-zA-Z0-9_]*)\}#', '(?P<$1>[^/]+)', $pattern) . '$#';
        $this->routes[] = [
            'method' => strtoupper($method),
            'regex' => $regex,
            'handler' => $handler,
        ];
        return $this;
    }

    public function get(string $pattern, callable $handler): self
    {
        return $this->add('GET', $pattern, $handler);
    }

    public function post(string $pattern, callable $handler): self
    {
        return $this->add('POST', $pattern, $handler);
    }

    public function dispatch(string $method, string $path): void
    {
        $method = strtoupper($method);
        foreach ($this->routes as $route) {
            if ($route['method'] !== $method) {
                continue;
            }
            $matches = [];
            if (preg_match($route['regex'], $path, $matches) === 1) {
                $params = array_filter($matches, fn($k) => is_string($k), ARRAY_FILTER_USE_KEY);
                ($route['handler'])($params);
                return;
            }
        }
        http_response_code(404);
        header('Content-Type: application/json');
        echo json_encode(['error' => 'Not Found', 'path' => $path]);
    }
}
