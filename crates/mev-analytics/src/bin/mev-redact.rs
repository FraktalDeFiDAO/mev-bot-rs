//! Filter diagnostic logs through the same URL credential redactor used by the bot.
use std::io::{BufRead, Write};

fn main() -> std::io::Result<()> {
    let input = std::io::stdin();
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    for line in input.lock().lines() {
        writeln!(output, "{}", mev_types::redact_secrets(&line?))?;
    }
    output.flush()
}
