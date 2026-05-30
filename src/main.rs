use std::error::Error;
use std::path::PathBuf;

use postgres::{Client, NoTls, SimpleQueryMessage};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::{Context, Editor};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

/// Stub vocabulary used for tab completion. Real commands come later.
const KEYWORDS: &[&str] = &[
    "select", "from", "where", "insert", "update", "delete", "create", "table", "\\q", "\\h",
    "\\d", "\\l",
];

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

/// Connect using the first CLI argument or `DATABASE_URL` as a libpq
/// connection string. Returns `None` when no connection info is supplied, so
/// the REPL still runs offline. Exits the process if a string was given but
/// the connection failed.
fn connect() -> Option<Client> {
    let conn = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("DATABASE_URL").ok());

    let Some(conn) = conn else {
        println!("not connected. Pass a connection string as an argument or set DATABASE_URL.");
        return None;
    };

    match Client::connect(&conn, NoTls) {
        Ok(client) => {
            println!("connected.");
            Some(client)
        }
        Err(err) => {
            eprintln!("connection failed: {err}");
            std::process::exit(1);
        }
    }
}

/// Run one SQL string (possibly several statements) and print each result.
/// `simple_query` returns every value as text, which is what a generic client
/// needs since result types are not known ahead of time.
fn run_sql(client: &mut Client, sql: &str) {
    let messages = match client.simple_query(sql) {
        Ok(messages) => messages,
        Err(err) => {
            // The useful detail (e.g. "relation does not exist") lives in the
            // error's source chain, not its top-level Display.
            eprint!("error: {err}");
            let mut source = err.source();
            while let Some(cause) = source {
                eprint!(": {cause}");
                source = cause.source();
            }
            eprintln!();
            return;
        }
    };

    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();

    for msg in &messages {
        match msg {
            SimpleQueryMessage::Row(row) => {
                if headers.is_empty() {
                    headers = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                let cells = (0..row.columns().len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect();
                rows.push(cells);
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

fn main() -> rustyline::Result<()> {
    let mut client = connect();
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
                    Some(c) => run_sql(c, trimmed),
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
    use super::{expects_table, format_table};

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
}
