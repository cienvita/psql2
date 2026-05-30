# psql2

An attempt to make a psql client with tab completions.

Early work in progress. A REPL shell: a `#` prompt with tab completion
against a stub keyword list, command history persisted to
`~/.psql2_history`, and `\q` to quit. It can connect to PostgreSQL and run
queries, printing results as an aligned table.

## Usage

Pass a libpq connection string as the first argument, or set `DATABASE_URL`:

    cargo run -- "host=localhost user=postgres dbname=postgres"
    DATABASE_URL="postgres://user:pw@localhost/db" cargo run

With no connection string it starts offline (completion and history only).
Type a few letters and press Tab to complete. `\q` or Ctrl-D exits,
Ctrl-C clears the current line.

## Non-interactive use

For scripts and tools, run a single command and exit:

    psql2 -c "select id, email from users" "host=localhost dbname=db"
    psql2 -c "select id, email from users" --json    # uses DATABASE_URL

With `--json`, row data is written to stdout as a JSON array of objects;
diagnostics and errors go to stderr, and the exit code is non-zero on a
database error.

Schema discovery (always JSON on stdout):

    psql2 --schema                 # list tables with column counts
    psql2 --schema users orders    # columns, types, primary key, foreign keys

When using `--schema` with table names, supply the connection via
`DATABASE_URL` (a positional connection string would be consumed as a table
name). Foreign-key output assumes single-column keys.

Connections are made without TLS for now, so servers that require SSL are
not yet supported.

## License

Dual-licensed under either of MIT or Apache-2.0, at your option.
