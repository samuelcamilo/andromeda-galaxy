# Andromeda Galaxy

> A high-throughput Ethereum ERC-20 deployment scanner and on-chain risk analyzer, written in Rust.

[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)
[![Actix Web](https://img.shields.io/badge/Actix--Web-4.9-blue.svg)](https://actix.rs/)
[![ethers-rs](https://img.shields.io/badge/ethers--rs-2.0-purple.svg)](https://github.com/gakonst/ethers-rs)

Andromeda Galaxy is an asynchronous bot/API built in Rust with Actix Web. It monitors the
Ethereum mainnet over a WebSocket RPC, detects newly created ERC-20 contracts in fresh blocks,
enriches the contract data (source, taxes, ownership, funding, on-chain simulation) and pushes a
formatted alert to a Telegram chat. It then keeps watching flagged tokens and edits the original
message when a contract turns into a rug pull or honeypot.

The bot is **read-only on mainnet**: it never signs transactions, holds no private key and performs
no deploys or trades. It only reads data from RPC/external APIs, runs buy/sell simulations on a
local Anvil fork, and writes to SQLite and Telegram.

---

## Highlights

- **~9,000 lines of idiomatic async Rust** organized in a clean controller / service / repository
  layering.
- **Resilient block ingestion** — a self-healing WebSocket subscription with buffered channels and
  exponential backoff reconnection (up to 60s).
- **EVM bytecode analysis** — ERC-20 detection by function-selector inspection, `CREATE`/`CREATE2`
  handling via `debug_traceTransaction` call tracing, and proxy implementation resolution.
- **Local fork simulation** — spins up ephemeral Foundry Anvil forks to measure real buy/sell taxes
  and gas without touching mainnet.
- **Multi-source enrichment** — Etherscan, Honeypot.is, 4byte.directory and GeckoTerminal, combined
  with direct JSON-RPC calls.
- **Bounded concurrency everywhere** — configurable parallelism and timeouts for block processing,
  enrichment and simulation.

## Architecture

```
Ethereum WS RPC  ──►  EthersRepository (subscribe_blocks, backoff)
                          │  buffered channel
                          ▼
              ListenDeployErc20ContractsService  ──►  FindDeploysService
                          │  (receipt / CREATE2 trace / proxy)      (ERC-20 selector check)
                          ▼
                  TelegramService queue  ──►  EnrichmentService  ──►  AnvilSimulation (fork)
                          │                        │                       (buy/sell tax + gas)
                          ▼                        ▼
                   Telegram alert            SQLite (history, tags, sent_messages)
                          ▲
              RugDetectorService (Honeypot.is polling, edits #RUGGED messages)
```

### Core components

| Component | Responsibility |
| --- | --- |
| `main.rs` | Bootstrap and dependency injection of every repository, service and controller. |
| `EthersRepository` | WebSocket providers per user, resilient block subscription with backoff. |
| `ListenDeployErc20ContractsService` | Consumes blocks, processes transactions in parallel, detects deploys. |
| `FindDeploysService` | ERC-20 detection via receipts, `CREATE2` tracing and proxy resolution. |
| `EnrichmentService` | Builds the `EnrichedDeploy` (source, taxes, owner, funding, checksums, socials). |
| `AnvilSimulation` | Local mainnet fork; simulates buy/sell to measure taxes and gas. |
| `TelegramService` | Message formatting (MarkdownV2 with plain-text fallback), queue and delivery. |
| `TelegramCommands` | `getUpdates` poll loop implementing the bot command set. |
| `RugDetectorService` | Background rug/honeypot re-check and message editing. |
| `SqliteRepository` | Schema and persistence (WAL, `synchronous=NORMAL`). |

## Tech stack

- **Language:** Rust 2021
- **HTTP/API:** `actix-web`
- **Ethereum:** `ethers-rs` (WebSocket, rustls)
- **Local storage:** SQLite via `rusqlite` (bundled)
- **Simulation:** Foundry Anvil (local mainnet fork)
- **Messaging:** Telegram Bot API
- **Bytecode analysis:** `revmasm`, `heimdall-cfg`
- **External APIs:** Etherscan, Honeypot.is, 4byte.directory, GeckoTerminal

## ERC-20 detection

A contract is treated as an ERC-20 when its runtime bytecode (fetched with `eth_getCode`) contains
the essential function selectors:

| Selector | Signature |
| --- | --- |
| `a9059cbb` | `transfer(address,uint256)` |
| `dd62ed3e` | `allowance(address,address)` |
| `095ea7b3` | `approve(address,uint256)` |
| `23b872dd` | `transferFrom(address,address,uint256)` |

The primary path reads the transaction receipt's `contract_address` for standard `CREATE` deploys.
A secondary path detects `CREATE2` patterns in the transaction bytecode, calls
`debug_traceTransaction` with the `callTracer`, and walks `CREATE`/`CREATE2` frames. When a proxy is
deployed, `ProxyUtils` resolves the implementation address so the correct bytecode is validated.

## Enrichment output

Each valid deploy is enriched into an `EnrichedDeploy` containing, when available: name, symbol,
decimals, total supply, deployer (balance / nonce / funding source), buy & sell fees, max tx and max
wallet, estimated buy/sell gas, owner and renounced status, Etherscan verification and compiler
version, social links from verified source, the Uniswap V2 pair vs. WETH, bytecode/function
checksums, non-standard selectors, plus tags and annotations stored in SQLite.

## Rug / honeypot detection

`RugDetectorService` runs in the background. Every 30 seconds it pulls up to 10 not-yet-flagged
messages from `sent_messages`, queries `https://api.honeypot.is/v2/IsHoneypot`, and flags a token as
scam when the simulation reports a honeypot, the risk summary is `honeypot`, or the pair's ETH/WETH
reserve drops below `0.001 ETH`. Flagged messages are re-rendered and the original Telegram message
is edited in place with a `#RUGGED` prefix and struck-through blocks. The bot avoids false flags when
the API fails or a response can't be parsed reliably.

## Getting started

### Prerequisites

- Rust (2021 edition) and Cargo
- [Foundry](https://book.getfoundry.sh/) (`anvil` on `PATH`) for fork simulation
- An Ethereum RPC endpoint (WebSocket + HTTP)
- A Telegram bot token and an Etherscan API key

### Configuration

Copy the template and fill in your values. **Never commit your `.env`** — it is git-ignored.

```bash
cp env-example .env
```

```env
SQLITE_PATH=/app/data/andromeda.db
RUST_LOG=info
RPC_ENDPOINT=ws://your-rpc-host:8546
RPC_HTTP_ENDPOINT=http://your-rpc-host:8545
TELEGRAM_BOT_TOKEN=replace-with-new-bot-token
TELEGRAM_CHAT_ID=replace-with-chat-id
TELEGRAM_BOT_USERNAME=replace-with-bot-username
ETHERSCAN_API_KEY=replace-with-etherscan-api-key
```

| Variable | Role |
| --- | --- |
| `RPC_ENDPOINT` | WebSocket endpoint to subscribe to new blocks and query receipts/code. |
| `RPC_HTTP_ENDPOINT` | HTTP endpoint used by enrichment, comparison and the Anvil fork. |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token. |
| `TELEGRAM_CHAT_ID` | Chat/channel/group where alerts are sent. |
| `TELEGRAM_BOT_USERNAME` | Username used to build command links inside messages. |
| `ETHERSCAN_API_KEY` | Source code, verification status, contract creation and funding lookups. |
| `SQLITE_PATH` | SQLite file for history, tags, annotations and message tracking. |

Optional tuning: `TELEGRAM_ENRICH_CONCURRENCY` (default `4`), `TELEGRAM_ENRICH_TIMEOUT_SECS`
(default `180`, min `30`), `ANVIL_SIM_CONCURRENCY` (default `2`), `ANVIL_SIM_TIMEOUT_SECS`
(default `75`, min `10`).

### Run with Docker (recommended)

```bash
docker compose up --build
```

`entrypoint.sh` boots the server, waits for `/health`, applies the RPC endpoints, configures
Telegram and activates the ERC-20 deploy listener. The API is published on `127.0.0.1:8080` only.

### Run locally

```bash
cargo build --release
./target/release/andromeda-galaxy
# then drive the API (see startup.sh) to apply RPC, configure Telegram and start listening
```

## Telegram commands

| Command | Description |
| --- | --- |
| `/check <ca>` | Manually enrich a contract and reply with the standard alert card. |
| `/compare <addr>` | Find similar contracts in the local history. |
| `/compare <addrA> <addrB>` | Compare two contracts by bytecode / checksum / selectors. |
| `/add <address> <checksum> <tag>` | Add a tag/indicator for a checksum. |
| `/addBatch <address> <checksums...> <tag>` | Apply one tag to several checksums. |
| `/del <checksum>` | Remove a tag/indicator. |
| `/clear` | Clear all indicators. |
| `/anote …` / `/anoteGas …` | Save an annotation / gas annotation for a checksum. |
| `/anoteAppend …` / `/anoteGasAppend …` | Append text to an existing annotation. |
| `/ignore <checksum>` / `/rmignore <checksum>` | Manage the ignore list. |
| `/setsigma <username>` / `/setbanana <username>` | Save Sigma / Banana usernames. |

## HTTP API (selected endpoints)

- `GET|POST /health` — liveness probe.
- `POST /ethers/{id}/apply_rpc` — connect a WS RPC and start block subscription.
- `POST /ethers/{id}/listen_deploy_erc20` — consume the block queue and detect ERC-20 deploys.
- `POST /ethers/{id}/get_logs` — scan a block range and persist detected deploys.
- `GET /ethers/{id}/get_code/{address}` — return bytecode for an address.
- `POST /telegram/configure` — set bot token, chat id, username and Etherscan key; start command poll.
- `POST /telegram/test` — send a test message to the configured chat.
- Anvil helpers under `/anvil/*` — create/remove forks, set balance, simulate, mine, query nonce.

## Database

SQLite tables are created automatically: `labels`, `checksum_contracts`, `checksum_history`,
`signatures`, `erc20_deployments`, `indicators`, `annotations`, `gas_annotations`, `ignores`,
`bot_settings` and `sent_messages`. The database runs in WAL mode with `synchronous=NORMAL`.

## Security notes

- Do **not** commit `.env`, SQLite databases, logs, `target/` or `.git/`.
- The API has no built-in authentication — keep it on localhost or behind an authenticating proxy.
- Restrict private RPC endpoints at the provider/firewall level.
- The bot needs no private key or mnemonic. Any future feature requiring real signing must be
  documented and isolated.

## Project layout

```
src/
├── main.rs                       # bootstrap & DI
├── controllers/                  # HTTP layer (/ethers, /telegram, /anvil, /elastic, /heimdall …)
├── services/
│   ├── enrichment_service.rs     # enrichment, checksums, compare, /check
│   ├── anvil_simulation.rs       # local fork buy/sell simulation
│   ├── telegram_service.rs       # formatting / delivery / editing
│   ├── telegram_commands.rs      # Telegram command loop
│   ├── rug_detector_service.rs   # rug/honeypot re-processing
│   └── ethers/                   # block ingestion & deploy detection
├── repositories/
│   ├── ethers/                   # WS providers, block subscription, Anvil instances
│   └── sqlite_repository.rs      # schema & persistence
└── utils/                        # bytecode, proxy and disassembly helpers
```
