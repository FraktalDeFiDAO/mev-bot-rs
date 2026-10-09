//! How rows and tables read in a terminal.

/// `println!` that exits quietly when the reader has gone (`seqscope … | head`) instead of panicking.
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout().lock(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

use seqscope_core::Row;

/// `HH:MM:SS.mmm` (UTC) from unix milliseconds.
pub fn clock(t_ms: u64) -> String {
    let s = t_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        (s / 3600) % 24,
        (s / 60) % 60,
        s % 60,
        t_ms % 1000
    )
}

/// `0x1234…abcd`.
pub fn short(a: &str) -> String {
    if a.len() > 14 {
        format!("{}…{}", &a[..8], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

/// One line per row.
pub fn row_line(r: &Row) -> String {
    let mut s = format!(
        "{} seq {} {:<15} {:<14} to {} sel {}",
        clock(r.t_ms),
        r.seq,
        r.kind,
        r.protocol.as_deref().unwrap_or("-"),
        r.to.as_deref().map_or("-".to_string(), short),
        r.selector.as_deref().unwrap_or("-"),
    );
    if !r.via.is_empty() {
        s.push_str(&format!(" via {}", r.via.join(">")));
    }
    for (k, v) in &r.fields {
        s.push_str(&format!(
            " {k}={}",
            if v.len() > 24 { short(v) } else { v.clone() }
        ));
    }
    s.push_str(&format!(
        " [{}/{}{}{}]",
        r.profile,
        r.rule,
        r.tag.as_deref().map_or(String::new(), |t| format!(" #{t}")),
        if r.alert { " ALERT" } else { "" }
    ));
    s
}

/// A padded text table; cells are cut at 60 characters.
pub fn table(columns: &[String], rows: &[Vec<serde_json::Value>]) {
    let cell = |v: &serde_json::Value| {
        let s = match v {
            serde_json::Value::Null => "-".to_string(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if s.chars().count() > 60 {
            format!("{}…", s.chars().take(59).collect::<String>())
        } else {
            s
        }
    };
    let cells: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(cell).collect()).collect();
    let widths: Vec<usize> = (0..columns.len())
        .map(|i| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain([columns[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |vals: &[String]| {
        vals.iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
    };
    say!("{}", line(columns));
    say!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for r in &cells {
        say!("{}", line(r));
    }
}
