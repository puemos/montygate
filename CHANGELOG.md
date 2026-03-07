# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2024-12-01

### Added

- Sandboxed Python execution via Monty interpreter (v0.0.4)
- `tool()` builtin for single tool calls with named arguments
- `batch_tools()` builtin for parallel dispatch of independent calls
- Input variable injection to avoid inlining large values in scripts
- Tool registry with keyword substring search and relevance scoring
- Policy engine with allow/deny/require-approval rules and wildcard patterns
- Rate limiting per tool via `N/sec`, `N/min`, `N/hour`, `N/day` syntax
- Scheduler with semaphore concurrency, timeout, and retry with exponential backoff
- Resource limits: execution time, memory, stack depth, external calls, code length
- Execution trace recording every tool call, duration, and retries
- NAPI-RS bindings for Node.js (macOS ARM64, macOS x64, Linux x64)
- TypeScript SDK with `Montygate` class
- Adapters for Anthropic, OpenAI, and Vercel AI SDK
- Auto-detection of tool definition formats (OpenAI, Anthropic, Vercel AI, OpenAI Agents SDK)
- Canonical LLM system prompt and JSON Schemas generated from Rust core
- 182 Rust tests (165 unit + 17 integration)
- 76 TypeScript tests (11 schema + 28 adapter + 11 integration + 26 e2e)

[0.1.0]: https://github.com/puemos/montygate/releases/tag/v0.1.0
