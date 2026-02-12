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

## What It Does

Each MCP tool call is a full LLM round-trip. Connecting multiple MCP servers means each interaction fans out into a chain of individual calls.

Montygate aggregates N downstream servers behind a single `run_program` tool. The LLM sends one call containing a Python script; that script can invoke any downstream tool via `tool("server.tool_name", ...)`. All calls execute in a single round-trip.

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

Instead of 5 sequential tool calls (5 round-trips), the LLM writes one `run_program` invocation:

```python
# Single run_program call replaces 3 round-trips
transcript = tool("gdrive.get_document", document_id="abc123")
summary = transcript[:500]

tool("salesforce.update_record",
    object_type="Lead",
    record_id="xyz",
    data={"notes": summary})

tool("slack.post_message",
    channel="#sales",
    text=f"Updated lead with {len(transcript)} char transcript")
```

## Quick Start

### Install

```bash
cargo install --git https://github.com/puemos/montygate.git montygate-cli
```

Or from a local clone:

```bash
git clone https://github.com/puemos/montygate.git
cd montygate
cargo install --path crates/montygate-cli
```

### Configure

```bash
# Initialize config
montygate config init

# Add downstream MCP servers
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

Or create `~/.montygate/config.toml` directly:

```toml
[server]
name = "montygate"
version = "0.1.0"

[[servers]]
name = "github"
[servers.transport]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
[servers.transport.env]
GITHUB_TOKEN = "${GITHUB_TOKEN}"

[[servers]]
name = "postgres"
[servers.transport]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres"]

[limits]
max_execution_time_ms = 30000
max_memory_bytes = 52428800
max_external_calls = 50

[policy.defaults]
action = "allow"

[[policy.rules]]
match_pattern = "*.delete_*"
action = "deny"
```

### Run

```bash
# Start with stdio transport (for Claude Desktop, Cursor, etc.)
montygate run

# Validate config without starting
montygate run --test-config

# List discovered tools
montygate run --list-tools
```

### Connect to Claude Desktop

Add to your Claude Desktop MCP config:

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

## Demo: Try It in 2 Minutes

No API keys needed. This demo uses three free official MCP servers:

| Server | Package | What it does |
|--------|---------|-------------|
| **fetch** | `@modelcontextprotocol/server-fetch` | Fetches and converts web pages to markdown |
| **memory** | `@modelcontextprotocol/server-memory` | Persistent knowledge graph storage |
| **everything** | `@modelcontextprotocol/server-everything` | Reference server with echo, add, and sample tools |

### 1. Set up

```bash
montygate config init
montygate server add fetch \
  --transport stdio \
  --command npx \
  --args "-y,@modelcontextprotocol/server-fetch"

montygate server add memory \
  --transport stdio \
  --command npx \
  --args "-y,@modelcontextprotocol/server-memory"

montygate server add everything \
  --transport stdio \
  --command npx \
  --args "-y,@modelcontextprotocol/server-everything"
```

### 2. Verify

```bash
montygate run --list-tools
```

You should see tools like `fetch.fetch`, `memory.create_entities`, `everything.echo`, etc.

### 3. Connect and use

Add Montygate to Claude Desktop (or any MCP client):

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

Now instead of 3 separate tool calls, the LLM writes a single `run_program`:

```python
# Fetch a webpage, extract key facts, and store them in memory
page = tool("fetch.fetch", url="https://modelcontextprotocol.io")
summary = page[:1000]

# Save to the knowledge graph
tool("memory.create_entities", entities=[
    {"name": "MCP", "entityType": "Protocol", "observations": [summary]}
])

# Verify it was stored
result = tool("memory.read_graph")

# Use the echo tool to confirm
tool("everything.echo", message=f"Stored {len(result)} entities")
```

3 servers, 4 tool calls, 1 round-trip. Without Montygate this would be 4 separate LLM round-trips.

> **Note on token savings:** Tool schemas are still communicated to the LLM (in the `run_program` tool description), so the up-front schema cost is relocated rather than eliminated. The primary wins are **batch execution** (N tool calls in 1 round-trip) and **programmatic orchestration** (conditionals, loops, data transformation between calls).

## Python Subset

Montygate uses the [Monty interpreter](https://github.com/pydantic/monty) (v0.0.4), a sandboxed Python implementation in Rust. It supports variables, arithmetic, strings, lists, dicts, control flow, function definitions, f-strings, try/except, comprehensions, slicing, `print()`, and `tool()` calls. User-defined classes and `import` of arbitrary modules are not available.

See [docs/python-support.md](docs/python-support.md) for the full reference.

## Architecture

### Crate Structure

```
montygate/
├── crates/
│   ├── montygate-core/       # Core library
│   │   ├── types.rs          # Shared types & error handling
│   │   ├── registry.rs       # Tool registry & Python stub generation
│   │   ├── engine.rs         # Execution engine abstraction
│   │   ├── bridge.rs         # Monty ↔ MCP dispatch bridge
│   │   └── policy.rs         # Access control & rate limiting
│   │
│   ├── montygate-mcp/        # MCP protocol layer
│   │   ├── mcp_server.rs     # Upstream MCP server (run_program tool)
│   │   ├── server.rs         # Server handler & builder
│   │   └── client_pool.rs    # Downstream client management
│   │
│   └── montygate-cli/        # CLI binary
│       ├── main.rs           # Entrypoint
│       ├── config.rs         # Config file management
│       └── commands/          # run, server, config subcommands
```

### Key Components

| Component | Purpose |
|-----------|---------|
| **Tool Registry** | Discovers and namespaces tools from downstream servers (`server.tool_name`). Auto-generates Python type stubs from JSON schemas. |
| **Policy Engine** | Per-tool allow/deny/require-approval rules with wildcard patterns (`*.delete_*`) and rate limiting (`10/min`). |
| **Execution Engine** | Trait-based abstraction. Ships with `MontyEngine` (real sandboxed Python execution via Monty) and `MockEngine` for testing. |
| **Bridge** | Connects code execution to MCP tool dispatch. Resolves tools, checks policies, dispatches calls, and records execution traces. |
| **MCP Server** | Exposes the single `run_program` tool via the [Model Context Protocol](https://modelcontextprotocol.io). |

### Data Flow

```
LLM sends run_program(code="...")
  → MontygateMcpServer receives the call
    → ExecutionEngine parses and runs the code
      → Code calls tool("github.create_issue", title="Bug")
        → Bridge resolves "github.create_issue" in ToolRegistry
          → PolicyEngine checks: allowed? rate-limited?
            → McpClientPool dispatches to downstream GitHub MCP server
              → Response flows back through the trace
```

## Configuration Reference

### Resource Limits

```toml
[limits]
max_execution_time_ms = 30000    # Max wall-clock time (default: 30s)
max_memory_bytes = 52428800      # Max memory usage (default: 50MB)
max_stack_depth = 100            # Max call stack depth
max_external_calls = 50          # Max tool calls per execution
max_code_length = 10000          # Max code size in characters
```

### Policy Rules

Rules are evaluated top-to-bottom. First match wins.

```toml
[policy.defaults]
action = "allow"                  # Default when no rule matches

[[policy.rules]]
match_pattern = "*.delete_*"      # Wildcard: block all delete operations
action = "deny"

[[policy.rules]]
match_pattern = "github.*"        # Server wildcard: rate-limit all GitHub tools
action = "allow"
rate_limit = "20/min"

[[policy.rules]]
match_pattern = "salesforce.update_record"  # Exact match: require approval
action = "require_approval"
```

**Pattern syntax:**
- `server.tool` &mdash; exact match
- `server.*` &mdash; all tools from a server
- `*.tool_*` &mdash; tool name pattern across all servers

**Rate limit syntax:**
- `N/sec`, `N/min`, `N/hour`, `N/day`

### Transport Options

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

## CLI Reference

```
montygate [OPTIONS] <COMMAND>

Options:
  -c, --config <FILE>     Config file path [default: ~/.montygate/config.toml]
  -l, --log-level <LEVEL> Log level [default: info]

Commands:
  run                     Start the MCP server
    -t, --transport       Transport: stdio | sse | http [default: stdio]
    --host                Host for SSE/HTTP [default: 127.0.0.1]
    --port                Port for SSE/HTTP [default: 8080]
    --test-config         Validate config and exit
    --list-tools          Show available tools and exit

  server add <NAME>       Add a downstream MCP server
  server remove <NAME>    Remove a server
  server list             List configured servers
  server edit <NAME>      Edit a server configuration
  server test <NAME>      Test server connectivity

  config init             Create default config file
  config show             Display current configuration
  config validate         Validate config file
```

## Security

Montygate applies multiple layers of protection:

- **Sandboxed Execution** &mdash; Code runs in an isolated interpreter with no filesystem, network, or environment access
- **Policy Engine** &mdash; Per-tool allow/deny/rate-limit rules evaluated before every call
- **Resource Limits** &mdash; CPU time, memory, stack depth, and call count are all bounded
- **Audit Trail** &mdash; Every external tool call is recorded in the execution trace with timing and arguments
- **Type Stubs** &mdash; Auto-generated from JSON schemas so the LLM produces well-typed calls

## Development

### Prerequisites

- Rust 1.85+ (edition 2024)
- Cargo

### Build

```bash
cargo build --release
```

### Test

```bash
# Run all 256 tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific crate
cargo test -p montygate-core

# Run specific test
cargo test test_bridge_dispatch_success
```

### Code Quality

```bash
cargo clippy              # Lint
cargo fmt                 # Format
cargo doc --open          # Generate docs
cargo tarpaulin           # Coverage report
```

### Project Stats

| Metric | Value |
|--------|-------|
| Total source lines | ~4,300 |
| Test count | 256 |
| Crates | 3 |
| Core library coverage | ~99% |

## Roadmap

- [x] Core architecture with trait-based engine design
- [x] Tool registry with Python stub generation from JSON schemas
- [x] Policy engine with wildcard patterns and rate limiting
- [x] MontyEngine with real Python execution via Monty interpreter
- [x] Mock execution engine for testing
- [x] MCP server with `run_program` tool via rmcp
- [x] CLI with server management and config commands
- [x] Comprehensive test suite (256 tests)
- [x] SSE and streamable HTTP transport support
- [x] Direct tool invocation escape hatch (`call_tool`)
- [x] Approval handler integration for human-in-the-loop workflows
- [ ] Full rmcp client integration for downstream tool calls
- [ ] Snapshot-based execution persistence (pause/resume)
- [ ] WebAssembly build for browser deployment

## Contributing

Contributions are welcome! Here's how to get started:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Write tests for your changes
4. Ensure `cargo test` and `cargo clippy` pass
5. Submit a pull request

Please read our code of conduct before contributing.

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.

## Acknowledgments

- Built with the official [rmcp](https://crates.io/crates/rmcp) MCP SDK for Rust
- Inspired by Pydantic's [Monty](https://github.com/pydantic/monty) Python interpreter
- [Model Context Protocol](https://modelcontextprotocol.io) by Anthropic
