# Python Subset Support (Monty v0.0.4)

MontyGate executes Python code using the [Monty interpreter](https://github.com/pydantic/monty), a sandboxed Python implementation written in Rust. This document describes what Python features are available.

## Supported

### Data Types
- **Integers** (arbitrary precision), **floats**, **booleans**, **None**
- **Strings** with full Unicode support
- **Lists**, **tuples**, **dicts**, **sets**, **frozensets**
- **Bytes** literals

### Control Flow
- `if` / `elif` / `else`
- `for` loops (iteration over any iterable)
- `while` loops with `break` / `continue`
- `pass` statement
- `try` / `except` / `finally` (full exception handling)

### Functions
- `def` with positional, default, `*args`, and `**kwargs` parameters
- `return` (including early returns)
- Nested functions and closures
- `lambda` expressions
- Generator functions with `yield` (basic support)

### Comprehensions
- List: `[x for x in items if cond]`
- Dict: `{k: v for k, v in items}`
- Set: `{x for x in items}`
- Nested comprehensions

### Operators
- Arithmetic: `+`, `-`, `*`, `/`, `//`, `%`, `**`
- Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`, `in`, `not in`, `is`
- Logical: `and`, `or`, `not`
- Bitwise: `&`, `|`, `^`, `~`, `<<`, `>>`
- Augmented assignment: `+=`, `-=`, `*=`, etc.
- Slicing: `[start:stop:step]`

### Strings
- F-strings: `f"{expr}"`, `f"{value:.2f}"`, `f"{x!r}"`
- String methods: `upper()`, `lower()`, `strip()`, `split()`, `join()`, `replace()`, `startswith()`, `endswith()`, `find()`, `format()`, etc.
- Concatenation, repetition, slicing

### Built-in Functions
`abs`, `all`, `any`, `bin`, `chr`, `divmod`, `enumerate`, `hash`, `hex`, `id`, `isinstance`, `len`, `min`, `max`, `next`, `oct`, `ord`, `pow`, `print`, `repr`, `reversed`, `round`, `sorted`, `sum`, `type`, `zip`

### Exception Types
All standard Python exceptions: `Exception`, `ValueError`, `TypeError`, `KeyError`, `IndexError`, `RuntimeError`, `ZeroDivisionError`, `AttributeError`, `NameError`, `StopIteration`, etc.

### Other
- Type hints and annotations (for optional type checking)
- Dataclasses (`@dataclass`)
- `async def` / `await` / `asyncio.gather()`

## Blocked

These are intentionally unavailable in the sandbox:

- **`import`** of arbitrary modules (only `sys`, `typing`, `asyncio`, `dataclasses` are available)
- **Filesystem access**: No `open()`, no `pathlib` (unless custom OS handler provided)
- **Network access**: No `socket`, `urllib`, `requests`
- **Environment access**: No `os.environ`, no `subprocess`
- **User-defined classes**: `class MyClass:` is not yet supported (use dataclasses instead)
- **`match` statements**: Pattern matching is not yet supported (use `if`/`elif`/`else`)
- **Threading / multiprocessing**

## Tool Calls

The primary way to interact with external services is the `tool()` function:

```python
result = tool("server.tool_name", arg1=val1, arg2=val2)
```

This calls a downstream MCP tool through MontyGate's bridge, which handles:
- Tool resolution in the registry
- Policy enforcement (allow/deny/rate-limit)
- Dispatch to the downstream MCP server
- Result conversion back to Python

## Resource Limits

All executions are bounded by configurable limits:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_execution_time_ms` | 30,000 | Wall-clock timeout |
| `max_memory_bytes` | 50 MB | Memory ceiling |
| `max_stack_depth` | 100 | Recursion depth |
| `max_external_calls` | 50 | Max `tool()` calls per execution |
| `max_code_length` | 10,000 | Max code size in characters |
