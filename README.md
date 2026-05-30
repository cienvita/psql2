# psql2

An attempt to make a psql client with tab completions.

Early work in progress. Right now it is a REPL shell: a `#` prompt with
tab completion against a stub keyword list, command history persisted to
`~/.psql2_history`, and `\q` to quit. There is no database connection yet.

## Usage

    cargo run

Type a few letters and press Tab to complete. `\q` or Ctrl-D exits,
Ctrl-C clears the current line.

## License

Dual-licensed under either of MIT or Apache-2.0, at your option.
