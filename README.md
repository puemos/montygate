<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.85+-orange?logo=rust" alt="Rust 1.85+" />
  <img src="https://img.shields.io/badge/npm-montygate-blue?logo=npm" alt="npm" />
  <a href="https://github.com/puemos/montygate/actions"><img src="https://img.shields.io/github/actions/workflow/status/puemos/montygate/ci.yml?branch=main&label=CI" alt="CI" /></a>
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-green" alt="License" /></a>
</p>

<h1 align="center">Montygate</h1>

<p align="center">
  Register tools once, execute scripts that orchestrate them, return one result.<br>
  A Rust + TypeScript library for sandboxed multi-tool orchestration.
</p>

---

## Why

Every tool call is a full LLM round-trip. Wire up a few tools and a simple workflow becomes five sequential calls, each burning tokens and latency.

Montygate collapses them. You register tools in TypeScript, and the LLM writes a short Python script that calls any of them via `tool()`, runs independent calls in parallel via `batch_tools()`, and glues results with variables, loops, and conditionals. N tool calls, one round-trip.

```
 ┌──────────────────────────────────────────────────┐
 │  Your App (Anthropic, OpenAI, Vercel AI, etc.)    │
 │  Sees: 2 tools -> execute + search                │
 └──────────────────┬─────────────────────────────────┘
                    │ function call
                    v
 ┌──────────────────────────────────────────────────┐
 │                  Montygate                        │
 │                                                   │
 │  ┌─────────────┐ ┌─────────────┐ ┌────────────┐  │
 │  │ Monty Engine│ │Tool Registry │ │  Policy    │  │
 │  │ (sandboxed) │ │ + Search    │ │  + Limits  │  │
 │  └──────┬──────┘ └─────────────┘ └────────────┘  │
 │         │                                         │
 │         │  tool('create_issue', title='Bug')      │
 │         v                                         │
 │  ┌───────────────────────────────────────────┐    │
 │  │      Your tool callbacks (JS/TS)          │    │
 │  │  ┌────────┐ ┌─────────┐ ┌──────────────┐ │    │
 │  │  │ GitHub │ │ DB      │ │ Slack        │ │    │
 │  │  └────────┘ └─────────┘ └──────────────┘ │    │
 │  └───────────────────────────────────────────┘    │
 └──────────────────────────────────────────────────┘
```

Works with any LLM framework via built-in adapters for Anthropic, OpenAI, and Vercel AI SDK.

## Quick Start

```bash
npm install montygate
```

```typescript
import { Montygate } from "montygate";
import { z } from "zod";

const gate = new Montygate();

gate.tool("lookup_order", {
  description: "Look up order details by order ID",
  params: z.object({ order_id: z.string() }),
  run: async ({ order_id }) => db.orders.find(order_id),
});

gate.tool("create_ticket", {
  description: "Create a support ticket",
  params: z.object({ subject: z.string(), body: z.string() }),
  run: async ({ subject, body }) => tickets.create({ subject, body }),
});

const result = await gate.execute(`
order = tool('lookup_order', order_id='ORD-123')
ticket = tool('create_ticket',
  subject='Late order ' + order['id'],
  body='Customer ' + order['email'] + ' has a late order'
)
ticket
`);

console.log(result.output); // Only this goes back to the LLM
```

### Use with an LLM adapter

```typescript
import { Montygate, toAnthropic, handleAnthropicToolCall } from "montygate";
import Anthropic from "@anthropic-ai/sdk";

const gate = new Montygate();
// ... register tools ...

const client = new Anthropic();
const tools = toAnthropic(gate);

const response = await client.messages.create({
  model: "claude-sonnet-4-20250514",
  max_tokens: 1024,
  tools,
  messages: [{ role: "user", content: "Look up order ORD-123 and create a ticket" }],
});

for (const block of response.content) {
  if (block.type === "tool_use") {
    const result = await handleAnthropicToolCall(gate, block.name, block.input);
    // Send result back to Claude...
  }
}
```

## Examples

### Sequential calls with data flowing between them

```python
order = tool('lookup_order', order_id='ORD-123')
summary = order['email'] + ': ' + str(order['items'])

tool('create_ticket',
    subject='Late order ' + order['id'],
    body=summary)

tool('send_email',
    to=order['email'],
    subject='Ticket created',
    text=f"Created ticket for order {order['id']}")
```

### Parallel dispatch when calls are independent

```python
results = batch_tools([
    ('get_weather', {'city': 'New York'}),
    ('get_weather', {'city': 'London'}),
    ('get_forecast', {'city': 'New York', 'days': 3}),
])
```

### Input variables to avoid inlining large values

```typescript
const result = await gate.execute(
  `result = tool('search_docs', query=search_term, limit=max_results)`,
  { search_term: "authentication", max_results: 10 }
);
```

### Tool discovery via keyword search

```typescript
const results = gate.search("create issue", 3);
// Returns matching tool definitions with names, descriptions, and input schemas
```

## Configuration

All configuration is programmatic via the `MontygateConfig` object:

```typescript
const gate = new Montygate({
  // Retry with exponential backoff for transient errors
  retry: {
    maxRetries: 3,
    baseDelayMs: 100, // 100ms, 200ms, 400ms, ...
  },

  // Execution limits
  limits: {
    timeoutMs: 30_000,
    maxConcurrent: 5,
  },

  // Sandbox resource limits
  resourceLimits: {
    maxExecutionTimeMs: 30_000,
    maxMemoryBytes: 52_428_800, // 50 MB
    maxStackDepth: 100,
    maxExternalCalls: 50,
    maxCodeLength: 10_000,
  },

  // Policy: first matching rule wins
  policy: {
    defaultAction: "allow",
    rules: [
      { matchPattern: "*.delete_*", action: "deny" },
      { matchPattern: "github.*", action: "allow", rateLimit: "20/min" },
      { matchPattern: "salesforce.update_record", action: "require_approval" },
    ],
  },
});
```

### Policy rules

Rules are evaluated top-to-bottom; first match wins. Patterns support:

- `create_issue` -- exact tool name
- `github.*` -- all tools matching a prefix (when using dotted names)
- `*.delete_*` -- wildcard across tool names

Rate limits: `N/sec`, `N/min`, `N/hour`, `N/day`.

## Adapters

Adapters convert a `Montygate` instance into tool definitions for your LLM framework. Each adapter exposes two tools to the LLM: `execute` (run a Python script) and `search` (discover tools by keyword).

### Anthropic

```typescript
import { toAnthropic, handleAnthropicToolCall } from "montygate";

const tools = toAnthropic(gate);  // AnthropicTool[]

// In the tool-use loop:
const result = await handleAnthropicToolCall(gate, block.name, block.input);
```

### OpenAI

```typescript
import { toOpenAI, handleOpenAIToolCall } from "montygate";

const tools = toOpenAI(gate);  // OpenAITool[]

// In the tool-call loop:
const result = await handleOpenAIToolCall(gate, call.function.name, call.function.arguments);
```

### Vercel AI SDK

```typescript
import { toVercelAI } from "montygate";

const tools = toVercelAI(gate);  // Record<string, VercelAIToolDef>

// Pass directly to generateText / streamText:
const { text } = await generateText({
  model: anthropic("claude-sonnet-4-20250514"),
  tools,
  prompt: "Search docs and summarize results",
});
```

## Python Subset

Montygate uses the [Monty interpreter](https://github.com/pydantic/monty) (v0.0.4), a sandboxed Python implementation in Rust. No filesystem, network, or import access.

**Supported:** variables, arithmetic, strings, lists, dicts, tuples, control flow (`if`/`elif`/`else`, `for`, `while`), function definitions, f-strings, try/except, list/dict/set comprehensions, slicing, `print()`, `tool()`, `batch_tools()`.

**Not supported:** `import`, user-defined classes, standard library modules.

## Architecture

```
montygate/
├── crates/
│   ├── montygate-core/    # Engine, registry, policy, scheduler, observability
│   └── montygate-napi/    # Node.js native bindings (NAPI-RS)
└── packages/
    └── montygate/         # TypeScript SDK + adapters
```

| Component | Description |
|---|---|
| **Engine** | Sandboxed Python execution via Monty with `tool()` and `batch_tools()` builtins |
| **Registry** | Tool catalog with keyword substring search and relevance scoring |
| **Policy** | Allow/deny/require-approval rules with wildcards and rate limiting |
| **Scheduler** | Concurrency limits, timeout, retry with exponential backoff |
| **Tracer** | Execution audit trail recording every tool call, duration, and retries |

```
execute(code, inputs)
  -> Engine parses + runs Python
    -> tool('create_issue', title='Bug')
    |   -> Registry resolves -> Policy checks -> Scheduler dispatches (with retry)
    |
    -> batch_tools([('a', {}), ('b', {})])
        -> All calls dispatched concurrently -> Results returned as list
```

## Security

- **Sandboxed execution** -- No filesystem, network, or environment access from scripts
- **Policy engine** -- Per-tool allow/deny/rate-limit evaluated before every call
- **Resource limits** -- CPU time, memory, stack depth, and call count bounded
- **Audit trail** -- Every tool call recorded in the execution trace

## Development

```bash
# Rust
cargo build --release
cargo test
cargo clippy && cargo fmt

# TypeScript SDK
pnpm install
pnpm build
pnpm test
```

## Roadmap

- [ ] Python SDK
- [ ] WebAssembly build for browser deployment

## Contributing

1. Fork and create a feature branch
2. Write tests for your changes
3. Ensure `cargo test` and `cargo clippy` pass
4. Submit a pull request

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Acknowledgments

- [Monty](https://github.com/pydantic/monty) -- Sandboxed Python interpreter by Pydantic
- [Model Context Protocol](https://modelcontextprotocol.io) by Anthropic -- inspiration for the tool orchestration pattern
