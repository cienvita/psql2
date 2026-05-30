use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::{Context, Editor};
use rustyline_derive::{Helper, Highlighter, Hinter, Validator};

/// Stub vocabulary used for tab completion. Real commands come later.
const KEYWORDS: &[&str] = &[
    "select", "from", "where", "insert", "update", "delete", "create", "table", "\\q", "\\h",
    "\\d", "\\l",
];

#[derive(Helper, Highlighter, Hinter, Validator)]
struct ReplHelper;

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

        let matches = KEYWORDS
            .iter()
            .filter(|kw| kw.starts_with(word))
            .map(|kw| Pair {
                display: kw.to_string(),
                replacement: kw.to_string(),
            })
            .collect();

        Ok((start, matches))
    }
}

fn main() -> rustyline::Result<()> {
    let mut rl: Editor<ReplHelper, _> = Editor::new()?;
    rl.set_helper(Some(ReplHelper));

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
                println!("you said: {trimmed}");
            }
            Err(ReadlineError::Interrupted) => continue, // Ctrl-C: clear line
            Err(ReadlineError::Eof) => break,            // Ctrl-D: quit
            Err(err) => {
                eprintln!("error: {err}");
                break;
            }
        }
    }

    Ok(())
}
