# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Purpose

Terminal UI app (TUI) for inspecting PSBT (Partially Signed Bitcoin Transactions) and building 2-of-2 multisig addresses, with an AI assistant powered by the Anthropic API. Built in Rust as an intern learning project covering ratatui, async/await, and Bitcoin protocol.

## Build & Run Commands

```bash
cargo build                  # compile
cargo run                    # run the TUI
cargo test                   # run all tests
cargo test <test_name>       # run a single test
cargo clippy                 # lint
cargo fmt                    # format
```

## Planned Dependencies (add to Cargo.toml as needed)

```toml
ratatui       = "0.29"
crossterm     = "0.28"
tokio         = { version = "1", features = ["full"] }
bitcoin       = { version = "0.32", features = ["serde"] }
reqwest       = { version = "0.12", features = ["json", "stream"] }
serde         = { version = "1", features = ["derive"] }
serde_json    = "1"
toml          = "0.8"
anyhow        = "1"
```

## Architecture

```
UI event loop (main thread, ratatui + crossterm)
        │
        ▼
  AppEvent enum  ◀──── mpsc::channel ◀──── Bitcoin task (tokio::spawn)
                                      ◀──── AI task     (tokio::spawn)
```

- **`src/main.rs`** — tokio runtime entry point, initializes terminal and starts the event loop
- **`src/app.rs`** — `AppState` struct and the main event loop; owns the mpsc receiver and dispatches `AppEvent` variants
- **`src/event.rs`** — `AppEvent` enum; keyboard input handler that converts crossterm events to `AppEvent`
- **`src/tui.rs`** — terminal init/teardown (alternate screen, raw mode)
- **`src/modules/bitcoin/psbt.rs`** — PSBT parsing via `bitcoin` crate, fee calculation, signing progress counter
- **`src/modules/bitcoin/multisig.rs`** — pubkey hex → redeem script (`OP_2 <pk1> <pk2> OP_2 OP_CHECKMULTISIG`) → P2WSH bech32 address; BIP67 key sorting
- **`src/modules/ui/inspector.rs`** — ratatui widgets for the PSBT inspector panel
- **`src/modules/ui/builder.rs`** — ratatui widgets for the multisig builder panel
- **`src/modules/ai/client.rs`** — HTTP call to Anthropic API
- **`src/modules/ai/stream.rs`** — streams AI response chunks into an mpsc channel for typewriter rendering
- **`src/modules/config/mod.rs`** — loads/saves `~/.config/psbt-inspector/config.toml` (API key, etc.)

## Key Behaviors

- `Tab` switches between **Inspector** and **Builder** panels
- `q` quits, arrow keys navigate, `?` opens the AI assistant
- UI loop never blocks: PSBT parsing and AI calls run in `tokio::spawn` tasks and send results back via `mpsc::channel`
- AI assistant receives the current PSBT summary + multisig detail as context on every call
- Config file at `~/.config/psbt-inspector/config.toml` stores the Anthropic API key

## Milestones

| Week | Goal |
|------|------|
| 1 | TUI skeleton — two panels, keyboard nav |
| 2 | PSBT parser — decode, display inputs/outputs/fee/signing progress |
| 3 | Multisig builder — pubkey → P2WSH address + descriptor |
| 4 | AI assistant — streaming response, tx context |
| 5 | Polish — error handling, config, README, publish |
