# hitagi /hi.ta.ɡi/

A minimalist Rust language server focused on low memory/CPU usage. Current features:

- Hover from open files only
- Diagnostics via `cargo check` on save
- Full text sync

## Build

```bash
cargo build --release
```

## Run

```bash
./target/release/hitagi
```

## Configuration

Settings are read from `hitagi` in your LSP client config:

- `workspaceMode`: `openFilesOnly`
- `checkOnSave`: `true` or `false`
- `checkCommand`: array of strings, defaults to `["cargo", "check", "-q", "--message-format=json"]`
- `logLevel`: `error|warn|info|debug`

## Notes

- Diagnostics are only published for currently open files.
- Hover looks for simple definitions in open files (e.g., `fn`, `struct`, `enum`).
