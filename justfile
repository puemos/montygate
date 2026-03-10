# Run all checks (lint + test)
check: lint test-rust test-node

# --- Rust ---

# Run Rust tests
test-rust:
    cargo test

# Run Rust tests for a specific crate
test-crate crate:
    cargo test -p {{crate}}

# Build Rust (debug)
build-rust:
    cargo build

# Build Rust (release)
build-rust-release:
    cargo build --release

# Run clippy
clippy:
    cargo clippy --workspace -- -D warnings

# --- Node ---

# Build NAPI bindings + TS SDK
build-node:
    cd packages/node && pnpm run build

# Build only NAPI bindings
build-napi:
    cd packages/node && pnpm run build:napi

# Build only TS SDK
build-ts:
    cd packages/node && pnpm run build:ts

# Run Node tests
test-node:
    cd packages/node && pnpm run test

# Run Node tests in watch mode
test-node-watch:
    cd packages/node && pnpm run test:watch

# Lint TS/JS
lint:
    cd packages/node && pnpm run lint

# Lint + fix TS/JS
lint-fix:
    cd packages/node && pnpm run lint:fix

# Run manual benchmark (requires ANTHROPIC_API_KEY)
manual-test:
    cd packages/node && npx tsx src/manual-agent-loop.ts

# --- Full build ---

# Build everything (Rust + NAPI + TS)
build: build-rust build-node

# Build everything in release mode
build-release: build-rust-release
    cd packages/node && pnpm run build:napi:release
    cd packages/node && pnpm run build:ts

# --- Site ---

# Run landing page dev server
site-dev:
    cd site && pnpm run dev

# Build landing page
site-build:
    cd site && pnpm run build

# Preview landing page build
site-preview:
    cd site && pnpm run preview

# Install dependencies
install:
    pnpm install
