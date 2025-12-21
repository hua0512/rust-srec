# rust-srec: 

WIP. Monorepo for a Rust implementation of stream-rec.

---

## 1. Core Applications

The project provides two main command-line applications: `strev-cli` and `mesio-cli`.

### `strev-cli`

Please read the [README](https://github.com/hua0512/rust-srec/blob/main/strev-cli/README.md)

### `mesio-cli` 

Please read the [README](https://github.com/hua0512/rust-srec/blob/main/mesio-cli/README.md)

---

## Examples

Webhook receiver (Axum):

- `cargo run -p rust-srec --example webhook_receiver`
- `cargo run -p rust-srec --example webhook_sender`
