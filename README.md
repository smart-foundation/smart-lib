# smart-lib

The Linux implementation of the [smart-protocol](https://github.com/smart-foundation/smart-protocol-official) contract.

## What this is

smart-lib is **one implementation** of the smart-protocol contract. It is not the implementation. Any third party can write their own implementation of the same contract, and every consumer (including smart-cli) will work identically.

## Architecture

```
smart-protocol (the contract)
       │
       ├── smart-lib (this repo — Linux implementation)
       │     ├── Engine traits + platform backends
       │     ├── Service that serves the contract over capnp RPC
       │     └── The coupled shell (being retired domain-by-domain)
       │
       └── smart-cli (contract-only consumer — does not know smart-lib)
```

## The contract is the API

Every domain, every method, every type is defined in [smart-protocol-official](https://github.com/smart-foundation/smart-protocol-official). This repo implements them. The schema is the authority — not this code.

## Building

```bash
# Build the library
cargo build -j6

# Build the daemon (serves the contract over capnp RPC)
cargo build -j6 --example smart-service --features rpc,std,domain-network

# Build with specific domains
cargo build -j6 --features domain-files,domain-process,domain-settings
```

## Testing

```bash
# Engine tests (one domain at a time — the full suite OOMs this box)
cargo test -j6 --lib --features domain-network
cargo test -j6 --lib --features domain-files

# Serve tests
cargo test -j6 --test serve_rpc

# Shell tests (one binary at a time)
cargo test -j6 -p shell --test network_cmd
```

## The coupled shell

`crates/shell/` contains the shell that is being retired domain-by-domain. Each domain that is ported to the contract loses its module here. When the last domain is ported, this crate is deleted.

The shell's presentation (tables, glyphs, truncation) is being ported to smart-cli's presenter framework.

## Key principles

1. **No shellouts.** Zero `Command::new` outside tests. Every backend reads the native mechanism.
2. **Honest availability.** Every method publishes REAL / DEGRADED / Unavailable with a measured reason. No fabricated numbers.
3. **Shellout-free by grep.** `grep -rn "Command::new" src/` must return nothing.
4. **No hardcoded paradigms.** Transports are open tokens, not closed enums. The transport set is extensible.
5. **The contract is the authority.** This code implements it; it does not define it.

## License

MIT OR Apache-2.0
