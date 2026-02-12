<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.85+-orange?logo=rust" alt="Rust 1.85+" />
  <img src="https://img.shields.io/badge/MCP-compatible-blue" alt="MCP Compatible" />
  <a href="https://github.com/puemos/montygate/actions"><img src="https://img.shields.io/github/actions/workflow/status/puemos/montygate/ci.yml?branch=main&label=CI" alt="CI" /></a>
  <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-green" alt="License" /></a>
</p>

<h1 align="center">Montygate</h1>

<p align="center">
  Aggregate N downstream MCP servers into a single <code>run_program</code> tool.<br>
  The LLM writes Python. Montygate executes it in a sandbox.
</p>

---

## Why

Every MCP tool call is a full LLM round-trip. Wire up three servers and a simple workflow becomes five sequential calls.

Montygate collapses them. It sits between the LLM and your downstream MCP servers, exposing one tool: `run_program`. The LLM sends a Python script that can call any downstream tool via `tool()`, run them in parallel via `batch_tools()`, and use variables, loops, and conditionals to glue results together. N calls, one round-trip.

```
 ┌──────────────────────────────────────────────────┐
 │  MCP Client (Claude Desktop, Cursor, Claude Code) │
 │  Sees: 1 tool → run_program                       │
 └──────────────────┬─────────────────────────────────┘
                    │ MCP protocol
                    ▼
 ┌──────────────────────────────────────────────────┐
 │                  Montygate                        │
 │                                                   │
 │  ┌────────────┐ ┌─────────────┐ ┌─────────────┐  │
 │  │ MCP Server │ │ Monty Engine│ │Tool Registry │  │
 │  │ (upstream) │ │ (sandboxed) │ │+ Policy      │  │
 │  └────────────┘ └─────────────┘ └─────────────┘  │
 │                                                   │
 │  ┌───────────────────────────────────────────┐    │
 │  │      MCP Client Pool (downstream)         │    │
 │  │  ┌────────┐ ┌─────────┐ ┌──────────────┐ │    │
 │  │  │ GitHub │ │Postgres │ │ Google Drive  │ │    │
 │  │  └────────┘ └─────────┘ └──────────────┘ │    │
 │  └───────────────────────────────────────────┘    │
 └──────────────────────────────────────────────────┘
```

## Examples

**Sequential calls with data flowing between them:**

```python
transcript = tool("gdrive.get_document", document_id="abc123")
summary = transcript[:500]

tool("salesforce.update_record",
    object_type="Lead", record_id="xyz",
    data={"notes": summary})

tool("slack.post_message",
    channel="#sales",
    text=f"Updated lead with {len(transcript)} char transcript")
```

**Parallel dispatch when calls are independent:**

```python
results = batch_tools([
    ("github.list_issues", {"repo": "foo/bar"}),
    ("github.list_issues", {"repo": "foo/baz"}),
    ("slack.list_channels", {}),
])
```

**Input variables to avoid inlining large values:**

```json
{
  "code": "result = tool('github.search', query=search_term, limit=max_results)",
  "inputs": {"search_term": "montygate", "max_results": 10}
}
```

> **Note:** Tool schemas are still sent to the LLM (in the `run_program` description), so the up-front token cost is relocated rather than eliminated. The wins are **batch execution** (N calls in 1 round-trip) and **programmatic orchestration** (conditionals, loops, data transformation).

## Quick Start

### Install

```bash
cargo install --git https://github.com/puemos/montygate.git montygate-cli
```

### Configure

```bash
montygate config init

montygate server add github \
  --transport stdio \
  --command npx \
  --args "-y,@modelcontextprotocol/server-github" \
  --env "GITHUB_TOKEN=${GITHUB_TOKEN}"

montygate server add postgres \
  --transport stdio \
  --command npx \
  --args "-y,@modelcontextprotocol/server-postgres"
```

### Run

```bash
montygate run                 # Start (stdio transport)
montygate run --list-tools    # Show discovered tools
montygate run --test-config   # Validate config and exit
```

### Connect to an MCP Client

Add to your Claude Desktop / Cursor / Claude Code config:

```json
{
  "mcpServers": {
    "montygate": {
      "command": "montygate",
      "args": ["run"]
    }
  }
}
```

## Configuration

All config lives in `~/.montygate/config.toml`. Override with `--config <path>`.

### Servers

```toml
# Stdio (spawn a child process)
[[servers]]
name = "github"
[servers.transport]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
[servers.transport.env]
GITHUB_TOKEN = "${GITHUB_TOKEN}"

# SSE (connect to a running server)
[[servers]]
name = "remote"
[servers.transport]
type = "sse"
url = "http://localhost:3001/sse"

# Streamable HTTP
[[servers]]
name = "api"
[servers.transport]
type = "streamable_http"
url = "http://localhost:8080/mcp"
```

### Resource Limits

```toml
[limits]
max_execution_time_ms = 30000    # Default: 30s
max_memory_bytes = 52428800      # Default: 50MB
max_stack_depth = 100
max_external_calls = 50
max_code_length = 10000
```

### Retry

Retries use exponential backoff and only trigger on transient errors (connection reset, timeout, broken pipe, connection refused, stream closed).

```toml
[retry]
max_retries = 3
retry_base_delay_ms = 100        # Backoff: 100ms, 200ms, 400ms, ...
connection_timeout_secs = 30
request_timeout_secs = 60
```

### Policy

Rules are evaluated top-to-bottom. First match wins.

```toml
[policy.defaults]
action = "allow"

[[policy.rules]]
match_pattern = "*.delete_*"                   # Block all delete operations
action = "deny"

[[policy.rules]]
match_pattern = "github.*"                     # Rate-limit all GitHub tools
action = "allow"
rate_limit = "20/min"

[[policy.rules]]
match_pattern = "salesforce.update_record"     # Require human approval
action = "require_approval"
```

Patterns: `server.tool` (exact), `server.*` (all tools from server), `*.tool_*` (across servers). Rate limits: `N/sec`, `N/min`, `N/hour`, `N/day`.

## Python Subset

Montygate uses the [Monty interpreter](https://github.com/pydantic/monty) (v0.0.4), a sandboxed Python implementation in Rust. Supports variables, arithmetic, strings, lists, dicts, control flow, function definitions, f-strings, try/except, comprehensions, slicing, `print()`, `tool()`, and `batch_tools()`. No `import` or user-defined classes.

See [docs/python-support.md](docs/python-support.md) for the full reference.

## Architecture

```
montygate/
├── crates/
│   ├── montygate-core/       # Engine, bridge, registry, policy, types
│   ├── montygate-mcp/        # MCP server (upstream) + client pool (downstream)
│   └── montygate-cli/        # CLI binary + config management
```

**Tool Registry** discovers and namespaces downstream tools (`server.tool_name`) with auto-generated Python type stubs. **Policy Engine** enforces per-tool allow/deny/require-approval rules with wildcards and rate limiting. **Execution Engine** runs sandboxed Python via Monty, with input variable injection and `batch_tools()` parallel dispatch. **Bridge** connects execution to MCP dispatch, resolving tools, checking policies, and recording traces. **Client Pool** manages downstream connections with retry and exponential backoff.

```
run_program(code="...", inputs={...})
  → Engine parses + runs Python
    → tool("github.create_issue", title="Bug")
    │   → Registry resolves → Policy checks → Client dispatches (with retry)
    │
    → batch_tools([("a.x", {}), ("b.y", {})])
        → All calls dispatched concurrently → Results returned as list
```

## Security

- **Sandboxed Execution** -- No filesystem, network, or environment access
- **Policy Engine** -- Per-tool allow/deny/rate-limit evaluated before every call
- **Resource Limits** -- CPU time, memory, stack depth, and call count bounded
- **Audit Trail** -- Every tool call recorded in the execution trace
- **Type Stubs** -- Auto-generated from JSON schemas for well-typed calls

## CLI Reference

```
montygate [OPTIONS] <COMMAND>

Options:
  -c, --config <FILE>     Config file path [default: ~/.montygate/config.toml]
  -l, --log-level <LEVEL> Log level [default: info]

Commands:
  run                     Start the MCP server
    -t, --transport       stdio | sse | http [default: stdio]
    --host / --port       For SSE/HTTP [default: 127.0.0.1:8080]
    --test-config         Validate and exit
    --list-tools          Show available tools and exit
  server add|remove|list|edit|test <NAME>
  config init|show|validate
```

## Development

```bash
cargo build --release     # Build
cargo test                # Run all tests
cargo clippy && cargo fmt # Lint + format
```

## Roadmap

- [ ] Full rmcp client integration for downstream tool calls
- [ ] Snapshot-based execution persistence (pause/resume)
- [ ] WebAssembly build for browser deployment

## Contributing

1. Fork and create a feature branch
2. Write tests for your changes
3. Ensure `cargo test` and `cargo clippy` pass
4. Submit a pull request

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Acknowledgments

- [rmcp](https://crates.io/crates/rmcp) -- MCP SDK for Rust
- [Monty](https://github.com/pydantic/monty) -- Sandboxed Python interpreter by Pydantic
- [Model Context Protocol](https://modelcontextprotocol.io) by Anthropic
