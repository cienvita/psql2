use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use postgres::{Client, NoTls, SimpleQueryMessage};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::{Context, Editor};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};
use serde_json::{json, Value};

/// Stub vocabulary used for tab completion. Real commands come later.
const KEYWORDS: &[&str] = &[
    "select", "from", "where", "insert", "update", "delete", "create", "table", "\\q", "\\h",
    "\\d", "\\l",
];

#[derive(Parser)]
#[command(version, about = "A psql client with tab completion")]
struct Cli {
    /// libpq connection string or URL. Falls back to DATABASE_URL.
    connection: Option<String>,

    /// Run a single SQL command and exit instead of starting the REPL.
    #[arg(short = 'c', long = "command", value_name = "SQL")]
    command: Option<String>,

    /// Describe schema and exit: with no names, list tables; with table
    /// names, show their columns, primary key, and foreign keys (JSON).
    #[arg(long, num_args = 0.., value_name = "TABLE")]
    schema: Option<Vec<String>>,

    /// Emit JSON instead of an aligned table (implied by --schema).
    #[arg(long)]
    json: bool,
}

/// Path to the history file: `.psql2_history` in the user's home directory,
/// falling back to the current directory if home can't be determined.
fn history_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".psql2_history")
}

/// True when the text immediately before the word under the cursor expects a
/// table name, i.e. the last token is a clause keyword that introduces one.
fn expects_table(prefix: &str) -> bool {
    let last = prefix.split_whitespace().next_back();
    matches!(
        last.map(str::to_ascii_lowercase).as_deref(),
        Some("from" | "join" | "into" | "update")
    )
}

#[derive(Helper, Highlighter, Hinter, Validator)]
struct ReplHelper {
    /// Table names loaded from the connected database, for completion.
    tables: Vec<String>,
}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Find the start of the word under the cursor.
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |i| i + 1);
        let word = &line[start..pos];

        let to_pair = |s: &str| Pair {
            display: s.to_string(),
            replacement: s.to_string(),
        };

        // After FROM (and similar), complete table names; otherwise keywords.
        let matches: Vec<Pair> = if expects_table(&line[..start]) {
            self.tables
                .iter()
                .filter(|t| t.starts_with(word))
                .map(|t| to_pair(t))
                .collect()
        } else {
            KEYWORDS
                .iter()
                .filter(|kw| kw.starts_with(word))
                .map(|kw| to_pair(kw))
                .collect()
        };

        Ok((start, matches))
    }
}

/// Load user table names from the catalog for completion. System schemas are
/// excluded. Returns empty on error so the REPL keeps working.
fn load_tables(client: &mut Client) -> Vec<String> {
    let sql = "SELECT table_name FROM information_schema.tables \
               WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
               ORDER BY table_name";
    match client.simple_query(sql) {
        Ok(messages) => messages
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
                _ => None,
            })
            .collect(),
        Err(err) => {
            eprintln!("warning: could not load schema for completion: {err}");
            Vec::new()
        }
    }
}

/// Print a database error and its source chain to stderr. The useful detail
/// (e.g. "relation does not exist") lives in the source, not the top Display.
fn print_db_error(err: &postgres::Error) {
    eprint!("error: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        eprint!(": {cause}");
        source = cause.source();
    }
    eprintln!();
}

/// Run one SQL string and print results to stdout. With `json`, emit a JSON
/// array of row objects (data only on stdout); otherwise an aligned table.
/// Returns `Err` on a database error so callers can set the exit code.
fn run_command(client: &mut Client, sql: &str, json: bool) -> Result<(), ()> {
    let messages = match client.simple_query(sql) {
        Ok(messages) => messages,
        Err(err) => {
            print_db_error(&err);
            return Err(());
        }
    };

    if json {
        let mut headers: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut affected = 0u64;
        for msg in &messages {
            match msg {
                SimpleQueryMessage::Row(row) => {
                    if headers.is_empty() {
                        headers = row.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    rows.push(
                        (0..row.columns().len())
                            .map(|i| row.get(i).map(str::to_string))
                            .collect(),
                    );
                }
                SimpleQueryMessage::CommandComplete(n) => affected = *n,
                _ => {}
            }
        }
        println!("{}", rows_to_json(&headers, &rows));
        if headers.is_empty() {
            eprintln!("rows affected: {affected}");
        }
    } else {
        print_results(&messages);
    }
    Ok(())
}

/// Print result messages as aligned tables and `OK (n)` lines on stdout.
fn print_results(messages: &[SimpleQueryMessage]) {
    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();

    for msg in messages {
        match msg {
            SimpleQueryMessage::Row(row) => {
                if headers.is_empty() {
                    headers = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                rows.push(
                    (0..row.columns().len())
                        .map(|i| row.get(i).map(str::to_string))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(affected) => {
                if headers.is_empty() {
                    println!("OK ({affected})");
                } else {
                    println!("{}", format_table(&headers, &rows));
                    headers.clear();
                    rows.clear();
                }
            }
            // SimpleQueryMessage is non-exhaustive; ignore anything new.
            _ => {}
        }
    }

    // A trailing result set with no CommandComplete (shouldn't normally happen).
    if !headers.is_empty() {
        println!("{}", format_table(&headers, &rows));
    }
}

/// Convert rows to a JSON array of objects keyed by column name. A NULL cell
/// (`None`) becomes JSON null. Kept free of database types so it is testable.
fn rows_to_json(headers: &[String], rows: &[Vec<Option<String>>]) -> Value {
    let objects: Vec<Value> = rows
        .iter()
        .map(|row| {
            let obj: serde_json::Map<String, Value> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let v = row
                        .get(i)
                        .and_then(|c| c.clone())
                        .map_or(Value::Null, Value::String);
                    (h.clone(), v)
                })
                .collect();
            Value::Object(obj)
        })
        .collect();
    Value::Array(objects)
}

/// Render rows as an aligned text table, psql-style. A NULL cell (`None`) is
/// shown blank. Kept free of database types so it can be unit tested.
fn format_table(headers: &[String], rows: &[Vec<Option<String>>]) -> String {
    fn cell(c: &Option<String>) -> &str {
        c.as_deref().unwrap_or("")
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell(c).chars().count());
        }
    }

    let pad = |s: &str, w: usize| format!(" {s:<w$} ");
    let mut out = String::new();

    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| pad(h, widths[i]))
        .collect();
    out.push_str(&header_line.join("|"));
    out.push('\n');

    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
    out.push_str(&sep.join("+"));
    out.push('\n');

    for row in rows {
        let line: Vec<String> = (0..headers.len())
            .map(|i| pad(cell(&row[i]), widths[i]))
            .collect();
        out.push_str(&line.join("|"));
        out.push('\n');
    }

    let n = rows.len();
    out.push_str(&format!("({n} row{})", if n == 1 { "" } else { "s" }));
    out
}

/// Schema discovery. With no table names, list user tables and their column
/// counts. With names, describe each table's columns, primary key, and foreign
/// keys. Always emits JSON to stdout.
fn run_schema(client: &mut Client, tables: &[String]) -> Result<(), ()> {
    let value = if tables.is_empty() {
        schema_list(client)
    } else {
        schema_detail(client, tables)
    };
    match value {
        Ok(value) => {
            println!("{value}");
            Ok(())
        }
        Err(err) => {
            print_db_error(&err);
            Err(())
        }
    }
}

fn schema_list(client: &mut Client) -> Result<Value, postgres::Error> {
    let rows = client.query(
        "SELECT t.table_schema, t.table_name, count(c.column_name)::int \
         FROM information_schema.tables t \
         LEFT JOIN information_schema.columns c \
           ON c.table_schema = t.table_schema AND c.table_name = t.table_name \
         WHERE t.table_schema NOT IN ('pg_catalog', 'information_schema') \
           AND t.table_type = 'BASE TABLE' \
         GROUP BY t.table_schema, t.table_name \
         ORDER BY t.table_schema, t.table_name",
        &[],
    )?;
    let tables: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "schema": r.get::<_, String>(0),
                "table": r.get::<_, String>(1),
                "columns": r.get::<_, i32>(2),
            })
        })
        .collect();
    Ok(Value::Array(tables))
}

fn schema_detail(client: &mut Client, tables: &[String]) -> Result<Value, postgres::Error> {
    let names = tables.to_vec();

    let col_rows = client.query(
        "SELECT table_schema, table_name, column_name, data_type, is_nullable \
         FROM information_schema.columns \
         WHERE table_name::text = ANY($1) \
           AND table_schema NOT IN ('pg_catalog', 'information_schema') \
         ORDER BY table_schema, table_name, ordinal_position",
        &[&names],
    )?;
    let pk_rows = client.query(
        "SELECT tc.table_schema, tc.table_name, kcu.column_name \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_name = tc.constraint_name \
           AND kcu.table_schema = tc.table_schema \
         WHERE tc.constraint_type = 'PRIMARY KEY' \
           AND tc.table_name::text = ANY($1) \
         ORDER BY kcu.ordinal_position",
        &[&names],
    )?;
    let fk_rows = client.query(
        "SELECT tc.table_schema, tc.table_name, kcu.column_name, \
                ccu.table_schema, ccu.table_name, ccu.column_name \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON kcu.constraint_name = tc.constraint_name \
           AND kcu.table_schema = tc.table_schema \
         JOIN information_schema.constraint_column_usage ccu \
           ON ccu.constraint_name = tc.constraint_name \
           AND ccu.table_schema = tc.table_schema \
         WHERE tc.constraint_type = 'FOREIGN KEY' \
           AND tc.table_name::text = ANY($1)",
        &[&names],
    )?;

    type Key = (String, String);
    let key = |r: &postgres::Row| -> Key { (r.get(0), r.get(1)) };

    let mut order: Vec<Key> = Vec::new();
    let mut columns: BTreeMap<Key, Vec<Value>> = BTreeMap::new();
    for r in &col_rows {
        let k = key(r);
        if !columns.contains_key(&k) {
            order.push(k.clone());
        }
        columns.entry(k).or_default().push(json!({
            "name": r.get::<_, String>(2),
            "type": r.get::<_, String>(3),
            "nullable": r.get::<_, String>(4) == "YES",
        }));
    }

    let mut primary: BTreeMap<Key, Vec<String>> = BTreeMap::new();
    for r in &pk_rows {
        primary.entry(key(r)).or_default().push(r.get(2));
    }

    let mut foreign: BTreeMap<Key, Vec<Value>> = BTreeMap::new();
    for r in &fk_rows {
        foreign.entry(key(r)).or_default().push(json!({
            "column": r.get::<_, String>(2),
            "references": {
                "schema": r.get::<_, String>(3),
                "table": r.get::<_, String>(4),
                "column": r.get::<_, String>(5),
            },
        }));
    }

    let described: Vec<Value> = order
        .iter()
        .map(|k| {
            json!({
                "schema": k.0,
                "table": k.1,
                "columns": columns.get(k).cloned().unwrap_or_default(),
                "primary_key": primary.get(k).cloned().unwrap_or_default(),
                "foreign_keys": foreign.get(k).cloned().unwrap_or_default(),
            })
        })
        .collect();
    Ok(Value::Array(described))
}

/// Resolve the connection string from the CLI argument or DATABASE_URL.
fn resolve_connection(cli: &Cli) -> Option<String> {
    cli.connection
        .clone()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Non-interactive modes require a connection and emit only data to stdout.
    if cli.schema.is_some() || cli.command.is_some() {
        let Some(conn) = resolve_connection(&cli) else {
            eprintln!("error: a connection is required (argument or DATABASE_URL)");
            return ExitCode::FAILURE;
        };
        let mut client = match Client::connect(&conn, NoTls) {
            Ok(client) => client,
            Err(err) => {
                eprintln!("connection failed: {err}");
                return ExitCode::FAILURE;
            }
        };
        let result = match &cli.schema {
            Some(tables) => run_schema(&mut client, tables),
            None => run_command(&mut client, cli.command.as_deref().unwrap(), cli.json),
        };
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(()) => ExitCode::FAILURE,
        };
    }

    match repl(resolve_connection(&cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Interactive REPL: connect if possible, then read-eval-print until `\q`.
fn repl(conn: Option<String>) -> rustyline::Result<()> {
    let mut client = match conn {
        Some(conn) => match Client::connect(&conn, NoTls) {
            Ok(client) => {
                println!("connected.");
                Some(client)
            }
            Err(err) => {
                eprintln!("connection failed: {err}");
                return Ok(());
            }
        },
        None => {
            println!("not connected. Pass a connection string or set DATABASE_URL.");
            None
        }
    };

    let tables = client.as_mut().map(load_tables).unwrap_or_default();
    if !tables.is_empty() {
        println!("{} table(s) available for completion.", tables.len());
    }

    let mut rl: Editor<ReplHelper, _> = Editor::new()?;
    rl.set_helper(Some(ReplHelper { tables }));

    let history = history_path();
    // Missing file on first run is expected; surface anything else.
    if let Err(err) = rl.load_history(&history) {
        if !matches!(&err, ReadlineError::Io(e) if e.kind() == std::io::ErrorKind::NotFound) {
            eprintln!(
                "warning: could not load history from {}: {err}",
                history.display()
            );
        }
    }

    println!("psql2. Type \\q to quit, Tab to complete.");

    loop {
        match rl.readline("# ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(trimmed)?;
                if trimmed == "\\q" {
                    break;
                }
                match client.as_mut() {
                    Some(c) => {
                        let _ = run_command(c, trimmed, false);
                    }
                    None => println!("not connected: {trimmed}"),
                }
            }
            Err(ReadlineError::Interrupted) => continue, // Ctrl-C: clear line
            Err(ReadlineError::Eof) => break,            // Ctrl-D: quit
            Err(err) => {
                eprintln!("error: {err}");
                break;
            }
        }
    }

    if let Err(err) = rl.save_history(&history) {
        eprintln!(
            "warning: could not save history to {}: {err}",
            history.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{expects_table, format_table, rows_to_json};
    use serde_json::json;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn expects_table_after_table_keywords() {
        assert!(expects_table("select * from "));
        assert!(expects_table("select * from users join "));
        assert!(expects_table("update "));
        assert!(expects_table("insert into "));
    }

    #[test]
    fn expects_table_is_case_insensitive() {
        assert!(expects_table("SELECT * FROM "));
    }

    #[test]
    fn no_table_completion_elsewhere() {
        assert!(!expects_table("select "));
        assert!(!expects_table("select * from users where "));
        assert!(!expects_table(""));
    }

    #[test]
    fn renders_header_separator_rows_and_footer() {
        let headers = vec!["id".to_string(), "name".to_string()];
        let rows = vec![vec![s("1"), s("alice")], vec![s("2"), None]];
        let expected = concat!(
            " id | name  \n",
            "----+-------\n",
            " 1  | alice \n",
            " 2  |       \n",
            "(2 rows)",
        );
        assert_eq!(format_table(&headers, &rows), expected);
    }

    #[test]
    fn widens_columns_to_longest_value() {
        let headers = vec!["x".to_string()];
        let rows = vec![vec![s("looooong")]];
        let out = format_table(&headers, &rows);
        assert!(out.starts_with(" x        \n"));
        assert!(out.contains(" looooong \n"));
    }

    #[test]
    fn uses_singular_row_in_footer() {
        let headers = vec!["x".to_string()];
        let rows = vec![vec![s("9")]];
        assert!(format_table(&headers, &rows).ends_with("(1 row)"));
    }

    #[test]
    fn null_renders_blank() {
        let headers = vec!["a".to_string()];
        let rows = vec![vec![None]];
        assert!(format_table(&headers, &rows).contains("   \n"));
    }

    #[test]
    fn json_rows_are_objects_with_null_for_missing() {
        let headers = vec!["id".to_string(), "name".to_string()];
        let rows = vec![vec![s("1"), s("alice")], vec![s("2"), None]];
        let expected = json!([
            {"id": "1", "name": "alice"},
            {"id": "2", "name": null},
        ]);
        assert_eq!(rows_to_json(&headers, &rows), expected);
    }

    #[test]
    fn json_empty_result_is_empty_array() {
        let headers: Vec<String> = vec![];
        let rows: Vec<Vec<Option<String>>> = vec![];
        assert_eq!(rows_to_json(&headers, &rows), json!([]));
    }
}
