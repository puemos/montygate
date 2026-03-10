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

### Bring your existing tools

Already have OpenAI, Anthropic, or Vercel AI tool definitions? Pass them directly — Montygate auto-detects the format:

```typescript
import { Montygate } from "montygate";

// Your existing OpenAI tools — no rewriting needed
const gate = new Montygate({
  tools: [
    {
      type: "function",
      function: {
        name: "lookup_order",
        description: "Look up order details by order ID",
        parameters: {
          type: "object",
          properties: { order_id: { type: "string" } },
          required: ["order_id"],
        },
      },
    },
    {
      type: "function",
      function: {
        name: "create_ticket",
        description: "Create a support ticket",
        parameters: {
          type: "object",
          properties: { subject: { type: "string" }, body: { type: "string" } },
          required: ["subject", "body"],
        },
      },
    },
  ],
  handlers: {
    lookup_order: async (args) => db.orders.find(args.order_id),
    create_ticket: async (args) => tickets.create(args),
  },
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

Works with Anthropic tools (`name` + `input_schema`), Vercel AI tools (object with `execute`), and more. See [Supported Formats](#supported-formats).

### Register with Zod schemas

Prefer typed schemas? The `.tool()` method still works:

```typescript
import { Montygate } from "montygate";
import { z } from "zod";

const gate = new Montygate();

gate.tool("lookup_order", {
  description: "Look up order details by order ID",
  params: z.object({ order_id: z.string() }),
  run: async ({ order_id }) => db.orders.find(order_id),
});
```

### Use with an LLM

```typescript
import { Montygate } from "montygate";
import Anthropic from "@anthropic-ai/sdk";

const gate = new Montygate();
// ... register tools ...

const client = new Anthropic();

const response = await client.messages.create({
  model: "claude-sonnet-4-20250514",
  max_tokens: 1024,
  tools: gate.anthropic(),
  messages: [{ role: "user", content: "Look up order ORD-123 and create a ticket" }],
});

for (const block of response.content) {
  if (block.type === "tool_use") {
    const result = await gate.handleToolCall(block.name, block.input);
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

## Supported Formats

`tools()` auto-detects these formats:

| Format | Detection signature | Handler source |
|--------|---------------------|----------------|
| OpenAI Chat Completions | `type === "function"` + `.function.name` | handler map |
| OpenAI Responses API | `type === "function"` + `.name` (flat) | handler map |
| Anthropic Messages API | `.name` + `.input_schema` | handler map |
| Anthropic `betaZodTool` | `.name` + `.inputSchema` (Zod) + `.run` | embedded `.run` |
| OpenAI Agents SDK | `.name` + `.parameters` + `.execute` | embedded `.execute` |
| Vercel AI SDK | `.description` + `.execute` (no `.name`) | embedded `.execute` |

```typescript
// Array of tools + handler map (OpenAI, Anthropic raw)
gate.tools(openaiTools, { get_weather: handler });

// Object keyed by name (Vercel AI style — handlers embedded)
gate.tools({ weather: vercelTool, search: vercelTool2 });

// Chain from multiple sources
gate
  .tools(openaiTools, handlers)
  .tools(vercelAITools)
  .tool("custom", { params: z.object({...}), run });

// Constructor shorthand
new Montygate({ tools: openaiTools, handlers: { get_weather: handler } });
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

## LLM Integration

Each method returns tool definitions in the right format + `handleToolCall()` dispatches calls back. Two tools are exposed to the LLM: `execute` (run a Python script) and `search` (discover tools by keyword).

### Anthropic

```typescript
const response = await client.messages.create({
  tools: gate.anthropic(),
  messages,
});

for (const block of response.content) {
  if (block.type === "tool_use") {
    const result = await gate.handleToolCall(block.name, block.input);
  }
}
```

### OpenAI

```typescript
const response = await client.chat.completions.create({
  tools: gate.openai(),
  messages,
});

for (const call of response.choices[0].message.tool_calls ?? []) {
  const result = await gate.handleToolCall(call.function.name, call.function.arguments);
}
```

### Vercel AI SDK

```typescript
const { text } = await generateText({
  model: anthropic("claude-sonnet-4-20250514"),
  tools: gate.vercelai(),
  prompt: "Search docs and summarize results",
});
```

## Performance

Montygate collapses N tool calls into 1 LLM round-trip. The savings grow with the number of tools:

```
+------------------------------+---------------+---------------+----------+
| Scenario                     |   Traditional |     Montygate |  Savings |
+------------------------------+---------------+---------------+----------+
| Customer Support (3 tools)   |               |               |          |
|   Round trips                |             4 |             2 |      50% |
|   Input tokens               |         3,693 |         1,140 |      69% |
|   Est. cost                  |       $0.0153 |       $0.0067 |      56% |
|   Est. latency               |       3,350ms |       1,650ms |      51% |
+------------------------------+---------------+---------------+----------+
| Complex Orchestration (5)    |               |               |          |
|   Round trips                |             6 |             2 |      67% |
|   Input tokens               |         8,090 |         1,216 |      85% |
|   Est. cost                  |       $0.0303 |       $0.0069 |      77% |
|   Est. latency               |       5,050ms |       1,650ms |      67% |
+------------------------------+---------------+---------------+----------+
| High Fan-Out (7 tools)       |               |               |          |
|   Round trips                |             8 |             2 |      75% |
|   Input tokens               |        13,343 |         1,178 |      91% |
|   Est. cost                  |       $0.0478 |       $0.0068 |      86% |
|   Est. latency               |       6,750ms |       1,650ms |      76% |
+------------------------------+---------------+---------------+----------+
```

*Cost model: Claude Sonnet ($3/MTok input, $15/MTok output), 800ms avg LLM latency. Traditional = N+1 round-trips with growing context. Montygate = 2 round-trips (script + answer). Run `cargo test benchmark_comparison -- --nocapture` or `pnpm test benchmark` to reproduce.*

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
