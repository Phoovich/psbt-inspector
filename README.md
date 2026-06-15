# PSBT Inspector

Terminal UI (TUI) for inspecting Bitcoin PSBTs and building 2-of-2 multisig addresses, with an AI assistant powered by the Anthropic API.

Built in Rust as an intern learning project covering ratatui, async/await, and the Bitcoin protocol.

---

## Quick Start

```bash
cargo build --release
cargo run
```

---

## Usage

### Keyboard navigation

| Key | Action |
|-----|--------|
| `Tab` | Switch between Inspector and Builder panels |
| `Esc` | Quit (from any panel) |
| `?` | Open the AI assistant overlay |
| `Enter` | Parse PSBT / build multisig address |
| `Ctrl+U` | Clear the current input field |

---

### Inspector — paste a PSBT

1. Run `cargo run`
2. The app opens on the **Inspector** tab
3. Paste a PSBT (base64 or hex) into the input box
4. Press `Enter` — the app decodes it and shows:
   - PSBT version (v0 / BIP-174 or v2 / BIP-370), input count, output count, fee, signing progress
   - Per-input: txid:vout, value, script type, address, partial signatures
   - Per-output: value, script type, address
   - Warnings (e.g. "1 of 2 inputs signed")

Both PSBT v0 (BIP-174) and PSBT v2 (BIP-370) are supported. PSBTv2 PSBTs omit the
global unsigned transaction and instead carry per-input/output fields
(`PSBT_GLOBAL_INPUT_COUNT`, `PSBT_IN_PREVIOUS_TXID`, `PSBT_OUT_AMOUNT`, etc.),
which are parsed by a small hand-written reader in
`src/modules/bitcoin/psbt_v2.rs`.

Example base64 PSBT (unsigned, 1-in 1-out):

```
cHNidP8BAAoCAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA
```

---

### Builder — construct a 2-of-2 multisig address

1. Press `Tab` to switch to the **Builder** panel
2. Paste two compressed public keys (66 hex chars each)
3. Press `Enter` — the app shows the P2WSH address, descriptor, and witness script

Example pubkeys (secp256k1 generator point G and 2G — for testing only):

```
0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
```

Keys are automatically sorted per BIP-67 before script construction.

---

### AI assistant

1. Press `?` from any panel to open the AI overlay
2. Type a question and press `Enter`
3. The current PSBT summary and multisig info are sent as context automatically

Example questions:
- "What does this transaction do?"
- "Is this fee too high?"
- "What is the difference between 2-of-2 and 2-of-3 multisig?"
- "Explain the signing progress"

Requires an API key (see Configuration below).

---

## Configuration

Create `~/.config/psbt-inspector/config.toml`:

```toml
api_key          = "sk-ant-..."   # Anthropic API key — required for AI assistant
network          = "testnet"      # bitcoin | testnet | signet | regtest
ai_model         = "claude-sonnet-4-5"
ai_send_context  = true           # set false to never send PSBT/multisig data to the AI
```

Environment variables override the config file:

| Variable | Overrides |
|----------|-----------|
| `PSBT_INSPECTOR_API_KEY` | `api_key` |
| `PSBT_INSPECTOR_NETWORK` | `network` |
| `PSBT_INSPECTOR_AI_MODEL` | `ai_model` |

If `api_key` is empty the AI assistant shows an actionable error instead of crashing.

The config file may contain your API key in plaintext. **Prefer
`PSBT_INSPECTOR_API_KEY`** so the key never touches disk. If
`config.toml` is readable by other users (e.g. mode `644`), a startup
warning is shown in the title bar — run `chmod 600
~/.config/psbt-inspector/config.toml` to fix it.

If `network` is set to an unrecognised value, a startup warning is shown
and the app falls back to `testnet` (it never silently uses an
unexpected network).

### AI privacy

When you ask the AI assistant a question while a PSBT or multisig address
is loaded, the app can send that context (input/output txids, values,
addresses, redeem script pubkeys) along with your question to the
Anthropic API. The first time this would happen in a session, you're
asked for consent (`[y/n]`); your answer applies for the rest of the
session only. Set `ai_send_context = false` to never send this context.

---

## Development

```bash
cargo build        # compile
cargo run          # run the TUI
cargo test         # run all tests (82 tests)
cargo clippy       # lint
cargo fmt          # format
```

---

## Architecture

```
UI event loop (main thread, ratatui + crossterm)
        │
        ▼
  AppEvent enum  ◀──── mpsc::channel ◀──── Bitcoin task (tokio::spawn)
                                      ◀──── AI task     (tokio::spawn)
```

- `src/main.rs` — tokio runtime entry point, panic hook restores terminal
- `src/app.rs` — `AppState`, main event loop, keyboard dispatch
- `src/event.rs` — `AppEvent` enum
- `src/tui.rs` — terminal init / teardown (alternate screen, raw mode)
- `src/modules/bitcoin/psbt.rs` — PSBT v0 (BIP-174) parsing, fee calculation, signing progress
- `src/modules/bitcoin/psbt_v2.rs` — PSBT v2 (BIP-370) manual parser
- `src/modules/bitcoin/multisig.rs` — pubkey → P2WSH address + BIP-67 sort
- `src/modules/ui/inspector.rs` — Inspector panel widgets
- `src/modules/ui/builder.rs` — Builder panel widgets
- `src/modules/ai/client.rs` — Anthropic Messages API HTTP call
- `src/modules/ai/context.rs` — builds plain-text context from current app state
- `src/modules/config/mod.rs` — loads `~/.config/psbt-inspector/config.toml`

UI loop never blocks: PSBT parsing and AI calls run in `tokio::spawn` tasks and send results back via `mpsc::channel`.

---

## Milestones

| Week | Goal |
|------|------|
| 1 | TUI skeleton — two panels, keyboard nav |
| 2 | PSBT parser — decode, display inputs/outputs/fee/signing progress |
| 3 | Multisig builder — pubkey → P2WSH address + descriptor |
| 4 | AI assistant — streaming response, tx context |
| 5 | Polish — error handling, config, README |
