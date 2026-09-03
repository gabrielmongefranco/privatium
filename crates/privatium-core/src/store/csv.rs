// Project:  Privatium™  |  File: crates/privatium-core/src/store/csv.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The CSV a snapshot carries (spec/protocol.md §5.1, tier 2 of §5.3): RFC 4180,
//           written and read here so that a NULL and an empty string stay different — a
//           NULL is nothing between the commas, an empty string is `""` — and a value with
//           a comma, a quote or a newline survives. No inference anywhere: every field is
//           text, and the caller types it from schema.sql.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

/// Write `rows` under `header`, one record per line, `\n`-terminated.
///
/// A field is quoted when it holds a comma, a quote, a CR or LF, or when it is the empty
/// string. Quotes inside are doubled. `None` is written as nothing at all.
pub(crate) fn write(
    path: &Path,
    header: &[&str],
    rows: impl Iterator<Item = Vec<Option<String>>>,
) -> io::Result<()> {
    let mut out = io::BufWriter::new(fs::File::create(path)?);
    write_record(&mut out, header.iter().map(|h| Some(*h)))?;
    for row in rows {
        write_record(&mut out, row.iter().map(Option::as_deref))?;
    }
    out.flush()
}

fn write_record<'a>(
    out: &mut impl io::Write,
    fields: impl Iterator<Item = Option<&'a str>>,
) -> io::Result<()> {
    for (index, field) in fields.enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        let Some(field) = field else {
            continue;
        };
        if field.is_empty() || field.contains([',', '"', '\n', '\r']) {
            out.write_all(b"\"")?;
            out.write_all(field.replace('"', "\"\"").as_bytes())?;
            out.write_all(b"\"")?;
        } else {
            out.write_all(field.as_bytes())?;
        }
    }
    out.write_all(b"\n")
}

/// A file read back: the header, then every record as `Some(text)` or `None` for an empty
/// unquoted field.
#[derive(Debug)]
pub(crate) struct Table {
    pub header: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// Parse a file written by [`write`], or by anything else that speaks RFC 4180.
pub(crate) fn read(path: &Path) -> Result<Table, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut records = parse(&text)?;
    if records.is_empty() {
        return Err("empty file: no header".to_owned());
    }
    let header: Vec<String> = records
        .remove(0)
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect();
    for (index, row) in records.iter().enumerate() {
        if row.len() != header.len() {
            return Err(format!(
                "record {}: {} field(s), header has {}",
                index + 1,
                row.len(),
                header.len()
            ));
        }
    }
    Ok(Table {
        header,
        rows: records,
    })
}

/// The state machine. A quoted field is `Some` even when empty; an unquoted one is `Some`
/// when it has text and `None` when it has none.
fn parse(text: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    let mut records = Vec::new();
    let mut record: Vec<Option<String>> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                other => field.push(other),
            }
            continue;
        }
        match ch {
            '"' if field.is_empty() && !quoted => {
                quoted = true;
                in_quotes = true;
            }
            '"' => return Err("a quote inside an unquoted field".to_owned()),
            ',' => {
                record.push(finish(&mut field, &mut quoted));
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(finish(&mut field, &mut quoted));
                records.push(std::mem::take(&mut record));
            }
            '\n' => {
                record.push(finish(&mut field, &mut quoted));
                records.push(std::mem::take(&mut record));
            }
            other => field.push(other),
        }
    }
    if in_quotes {
        return Err("unterminated quoted field".to_owned());
    }
    if !field.is_empty() || quoted || !record.is_empty() {
        record.push(finish(&mut field, &mut quoted));
        records.push(record);
    }
    Ok(records)
}

fn finish(field: &mut String, quoted: &mut bool) -> Option<String> {
    let value = std::mem::take(field);
    let was_quoted = std::mem::take(quoted);
    if value.is_empty() && !was_quoted {
        None
    } else {
        Some(value)
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Every value the property test throws at the CSV tier, round-tripped.
    #[test]
    fn nulls_empties_quotes_commas_and_newlines_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.csv");
        let rows = vec![
            vec![
                Some("a".to_owned()),
                Some("line one\nline \"two\", with comma".to_owned()),
                None,
            ],
            vec![
                Some("b".to_owned()),
                Some(String::new()),
                Some("NULL".to_owned()),
            ],
            vec![Some("c".to_owned()), None, Some("q't [br]".to_owned())],
            vec![
                Some("d".to_owned()),
                Some("\r\n".to_owned()),
                Some("x".to_owned()),
            ],
        ];
        write(&path, &["id", "name", "tags"], rows.clone().into_iter()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("id,name,tags\n"));
        assert!(text.contains("b,\"\",NULL\n"), "{text}");
        assert!(text.contains("c,,q't [br]\n"), "{text}");
        let table = read(&path).unwrap();
        assert_eq!(table.header, ["id", "name", "tags"]);
        assert_eq!(table.rows, rows);
    }

    #[test]
    fn a_ragged_or_broken_file_is_an_error_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.csv");
        fs::write(&path, "id,name\na,b,c\n").unwrap();
        assert!(read(&path).unwrap_err().contains("record 1"));
        fs::write(&path, "id,name\na,\"unterminated\n").unwrap();
        assert!(read(&path).unwrap_err().contains("unterminated"));
        fs::write(&path, "id,name\na,b\"c\n").unwrap();
        assert!(read(&path).is_err());
        fs::write(&path, "").unwrap();
        assert!(read(&path).unwrap_err().contains("header"));
        // A last line without its newline is still a record.
        fs::write(&path, "id,name\na,b").unwrap();
        assert_eq!(read(&path).unwrap().rows.len(), 1);
    }
}
