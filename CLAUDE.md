# psql2

A PostgreSQL client: interactive REPL with tab completion and history, plus
non-interactive `-c` / `--json` / `--schema` modes for scripts and tools.

## Build and test

    cargo build            # debug build
    cargo build --release
    cargo test             # unit tests, no database needed
    cargo clippy
    cargo fmt

Unit tests cover the pure helpers (table formatting, JSON conversion,
completion context). They do not touch a database.

## Running

    cargo run -- "host=localhost user=postgres dbname=db"   # REPL
    cargo run -- -c "select 1" --json                       # one-shot, JSON
    cargo run -- --schema users orders                      # schema (uses DATABASE_URL)

`--schema` with table names needs the connection in `DATABASE_URL`; a
positional connection string is otherwise consumed as a table name.

## Integration testing against a real Postgres

There is no test harness for live queries yet. To check behaviour by hand,
run a throwaway container:

    docker run -d --rm --name psql2-test -e POSTGRES_PASSWORD=pw \
      -p 55432:5432 postgres:16-alpine
    export DATABASE_URL="host=127.0.0.1 port=55432 user=postgres password=pw dbname=postgres"
    # ... exercise the binary ...
    docker stop psql2-test

## Notes

* Connections use `NoTls`; servers requiring SSL are not yet supported.
* On Windows a leftover `psql2.exe` from a piped run can hold a lock and make
  `cargo build` fail with "Access is denied". Kill it first:
  `taskkill /IM psql2.exe /F`.
* ASCII only in code, comments, and docs.
