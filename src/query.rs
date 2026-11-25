use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::{
    Connection,
    config::DbConfig,
    params_from_iter,
    types::{Value, ValueRef},
};
use std::collections::HashSet;
use std::sync::Arc;

pub type Batch = Arc<Vec<Vec<String>>>;

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn column_names(source: &[String]) -> Vec<String> {
    let mut used = HashSet::new();
    source
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let base = if name.trim().is_empty() {
                format!("column_{}", index + 1)
            } else {
                name.clone()
            };
            let mut result = base.clone();
            let mut suffix = 2;
            while !used.insert(result.to_ascii_lowercase()) {
                result = format!("{base}_{suffix}");
                suffix += 1;
            }
            result
        })
        .collect()
}

fn value(text: &str) -> Value {
    if text.is_empty() {
        return Value::Null;
    }
    if let Ok(number) = text.parse::<i64>()
        && number.to_string() == text
    {
        return Value::Integer(number);
    }
    if text.contains(['.', 'e', 'E'])
        && let Ok(number) = text.parse::<f64>()
        && number.is_finite()
        && number.to_string() == text
    {
        return Value::Real(number);
    }
    Value::Text(text.to_owned())
}

/// SQLite's empty filename creates a private temporary database, removed on close.
pub fn database(columns: &[String]) -> Result<Connection, String> {
    if columns.is_empty() {
        return Err("This source has no columns.".into());
    }
    let names = column_names(columns)
        .iter()
        .map(|n| ident(n))
        .collect::<Vec<_>>()
        .join(", ");
    let db = Connection::open("").map_err(|e| e.to_string())?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)
        .map_err(|e| e.to_string())?;
    db.execute_batch(&format!("CREATE TABLE data ({names})"))
        .map_err(|e| e.to_string())?;
    Ok(db)
}

pub fn add_columns(db: &Connection, names: &[String]) -> Result<(), String> {
    for name in names {
        db.execute_batch(&format!("ALTER TABLE data ADD COLUMN {}", ident(name)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn insert(db: &mut Connection, width: usize, rows: &[Vec<String>]) -> Result<(), String> {
    let args = vec!["?"; width].join(", ");
    let tx = db.transaction().map_err(|e| e.to_string())?;
    {
        let mut statement = tx
            .prepare_cached(&format!("INSERT INTO data VALUES ({args})"))
            .map_err(|e| e.to_string())?;
        for row in rows {
            statement
                .execute(params_from_iter(
                    (0..width).map(|i| value(row.get(i).map(String::as_str).unwrap_or(""))),
                ))
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

fn unprefix<'a>(text: &'a str, choices: &[&str]) -> Option<&'a str> {
    choices.iter().find_map(|word| text.strip_prefix(word))
}

/// Accept exact headers first, then unambiguous spoken versions such as "temp c".
fn resolve_column(name: &str, columns: &[String]) -> Result<String, String> {
    let name = name.trim();
    let quoted = name.strip_prefix('"').and_then(|n| n.strip_suffix('"'));
    let literal = quoted.unwrap_or(name).replace("\"\"", "\"");
    let key = literal.to_lowercase();
    if let Some(header) = columns.iter().find(|header| header.to_lowercase() == key) {
        return Ok(ident(header));
    }
    if quoted.is_none() {
        let spoken = |s: &str| {
            s.to_lowercase()
                .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let key = spoken(name);
        let mut candidates = columns.iter().filter(|n| spoken(n) == key);
        if let Some(first) = candidates.next() {
            if let Some(second) = candidates.next() {
                return Err(format!(
                    "“{name}” could mean “{first}” or “{second}”. Use the exact column name."
                ));
            }
            return Ok(ident(first));
        }
    }
    Err(format!(
        "Unknown column “{name}”. Use a column name from the table."
    ))
}

/// A quoted header may itself contain " by ".
fn split_group(text: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    for (index, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        if !quoted && let Some(group) = text[index..].strip_prefix(" by ") {
            return (&text[..index], Some(group));
        }
    }
    (text, None)
}

fn row_limit(number: &str) -> Result<u32, String> {
    number
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "Use a positive whole number of rows.".to_owned())
}

pub fn translate(input: &str, columns: &[String]) -> Result<String, String> {
    let input = input.trim();
    let lower = input.to_lowercase();
    let first = lower
        .split(|c: char| !c.is_alphabetic())
        .next()
        .unwrap_or("");
    if matches!(first, "select" | "with" | "values")
        || input.starts_with("--")
        || input.starts_with("/*")
    {
        return Ok(input.to_owned());
    }

    let question = lower.trim_end_matches(['?', '.']).trim_end();
    let question = unprefix(
        question,
        &[
            "what is the ",
            "what's the ",
            "show me the ",
            "show me ",
            "show the ",
            "show ",
        ],
    )
    .unwrap_or(question);
    let names = column_names(columns);
    let column = |name: &str| resolve_column(name, &names);

    if let Some(rest) = question.strip_prefix("first ")
        && let Some(amount) = rest
            .strip_suffix(" rows")
            .or_else(|| rest.strip_suffix(" row"))
    {
        let limit = row_limit(amount)?;
        return Ok(format!("SELECT * FROM data LIMIT {limit}"));
    }
    if let Some(rest) = unprefix(
        question,
        &[
            "how many rows are there",
            "how many rows",
            "count rows",
            "number of rows",
            "row count",
        ],
    ) {
        if rest.is_empty() {
            return Ok("SELECT COUNT(*) AS rows FROM data".into());
        }
        if let Some(group) = unprefix(rest, &[" grouped by ", " for each ", " per ", " by "]) {
            let name = column(group)?;
            return Ok(format!(
                "SELECT {name}, COUNT(*) AS rows FROM data GROUP BY {name} ORDER BY 2 DESC, 1"
            ));
        }
    }

    let frequency = |candidate: &str, limit: u32| {
        let name = column(candidate)?;
        Ok(format!(
            "SELECT {name}, COUNT(*) AS frequency FROM data WHERE {name} IS NOT NULL GROUP BY {name} ORDER BY 2 DESC, 1 LIMIT {limit}"
        ))
    };
    if let Some(rest) = unprefix(question, &["which ", "what "])
        && let Some(candidate) = rest
            .strip_suffix(" is most common")
            .or_else(|| rest.strip_suffix(" is the most common"))
    {
        return frequency(candidate, 1);
    }
    if let Some(rest) = question.strip_prefix("most common ") {
        return frequency(rest, 1);
    }
    if let Some(rest) = question.strip_prefix("top ") {
        let (limit, candidate) = match rest.split_once(' ') {
            Some((n, tail)) if n.starts_with(|c: char| c.is_ascii_digit() || c == '-') => {
                (row_limit(n)?, tail)
            }
            _ => (10, rest),
        };
        return frequency(
            unprefix(candidate, &["values in ", "values of "]).unwrap_or(candidate),
            limit,
        );
    }

    for (prefix, op) in [
        ("average ", "AVG"),
        ("mean ", "AVG"),
        ("sum ", "SUM"),
        ("total ", "SUM"),
        ("minimum ", "MIN"),
        ("maximum ", "MAX"),
    ] {
        if let Some(rest) = question.strip_prefix(prefix) {
            let (metric, group) = split_group(rest.strip_prefix("of ").unwrap_or(rest));
            let metric = column(metric)?;
            // JSON's number grammar recognizes decimal and exponent text while excluding padded IDs.
            // CASE also keeps malformed text out of json_type(), which requires valid JSON.
            let numeric = format!("CAST({metric} AS NUMERIC)");
            let aggregate = format!(
                "{op}({numeric}) AS result FROM data WHERE CASE WHEN json_valid({metric}) THEN json_type({metric}) IN ('integer','real') END AND {numeric} BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308"
            );
            return match group {
                Some(group) => {
                    let group = column(group)?;
                    Ok(format!(
                        "SELECT {group}, {aggregate} GROUP BY {group} ORDER BY 1"
                    ))
                }
                None => Ok(format!("SELECT {aggregate}")),
            };
        }
    }
    Err("Try “how many rows per COLUMN?”, “top 5 values in COLUMN”, “average COLUMN by GROUP”, “show first 5 rows”, or write SQL against data.".into())
}

pub fn select(
    db: &Connection,
    sql: &str,
    mut schema: impl FnMut(Vec<String>) -> Result<(), String>,
    mut row: impl FnMut(Vec<String>) -> Result<(), String>,
) -> Result<(), String> {
    db.execute_batch("PRAGMA query_only=ON")
        .map_err(|e| e.to_string())?;
    let mut batch = rusqlite::Batch::new(db, sql);
    let mut statement = batch
        .next()
        .map_err(|e| e.to_string())?
        .ok_or("Enter a query.")?;
    if batch.next().map_err(|e| e.to_string())?.is_some()
        || !statement.readonly()
        || statement.column_count() == 0
    {
        return Err("Enter a single read-only SELECT query.".into());
    }
    let count = statement.column_count();
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut records = statement.query([]).map_err(|e| e.to_string())?;
    let mut current = records.next().map_err(|e| e.to_string())?;
    schema(names)?;
    while let Some(record) = current {
        let mut fields = Vec::with_capacity(count);
        for i in 0..count {
            let cell = match record.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => String::new(),
                ValueRef::Integer(n) => n.to_string(),
                ValueRef::Real(n) => n.to_string(),
                ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                ValueRef::Blob(bytes) => format!("<{} bytes>", bytes.len()),
            };
            fields.push(cell);
        }
        row(fields)?;
        current = records.next().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod language_tests {
    use super::*;

    #[test]
    fn questions_execute_against_the_loaded_columns() {
        let columns = ["Habitat", "Temp_C"].map(String::from);
        let mut db = database(&columns).unwrap();
        let records: Vec<_> = [
            ["marsh", "10"],
            ["wood", "9"],
            ["marsh", "20"],
            ["wood", "n/a"],
            ["marsh", ""],
        ]
        .map(|row| row.map(String::from).to_vec())
        .into();
        insert(&mut db, 2, &records).unwrap();
        let results = |question| {
            let sql = translate(question, &columns).unwrap();
            let mut rows = Vec::new();
            select(
                &db,
                &sql,
                |_| Ok(()),
                |row| {
                    rows.push(row);
                    Ok(())
                },
            )
            .unwrap();
            rows
        };
        assert_eq!(
            results("How many rows per habitat?"),
            [["marsh", "3"], ["wood", "2"]]
        );
        assert_eq!(results("Which habitat is most common?"), [["marsh", "3"]]);
        assert_eq!(results("show me the first 1 row"), [["marsh", "10"]]);
        assert_eq!(
            results("What is the average temp c by habitat?"),
            [["marsh", "15"], ["wood", "9"]]
        );
    }

    #[test]
    fn projection_preserves_numeric_spellings_and_prompts_aggregate_decimals() {
        let columns = ["measurement", "identifier"].map(String::from);
        let mut db = database(&columns).unwrap();
        let rows: Vec<_> = [
            ["1.00", "00123"],
            ["0.25", "01.2"],
            ["1E1", "12345678901234567.01"],
            ["001", "1e30000"],
            ["missing", "000"],
        ]
        .map(|row| row.map(String::from).to_vec())
        .into();
        insert(&mut db, 2, &rows).unwrap();
        let run = |sql: &str| {
            let mut output = vec![];
            select(
                &db,
                sql,
                |_| Ok(()),
                |row| {
                    output.push(row);
                    Ok(())
                },
            )
            .unwrap();
            output
        };
        assert_eq!(run("SELECT * FROM data ORDER BY rowid"), rows);
        assert_eq!(
            run("SELECT typeof(measurement) FROM data WHERE measurement = 0.25"),
            [["real"]]
        );
        assert_eq!(
            run(&translate("sum measurement", &columns).unwrap()),
            [["11.25"]]
        );
    }

    #[test]
    fn quoted_column_missing_from_replay_input_is_an_error() {
        let db = database(&["new_name".to_owned()]).unwrap();
        let error = select(
            &db,
            r#"SELECT "original_name" FROM data"#,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.contains("no such column"), "{error}");
    }

    #[test]
    fn quoted_and_ambiguous_column_names() {
        let columns = ["a-b", "a_b", "q\"uote by name"].map(String::from);
        assert!(
            translate("average a b", &columns)
                .unwrap_err()
                .contains("could mean")
        );
        assert!(
            translate("average missing", &columns)
                .unwrap_err()
                .contains("Unknown column")
        );
        assert!(
            translate("average \"q\"\"uote by name\" by \"a_b\"", &columns)
                .unwrap()
                .contains("AVG(CAST(\"q\"\"uote by name\" AS NUMERIC))")
        );
        assert!(translate("show first -2 rows", &columns).is_err());
        assert_eq!(
            translate("SELECT 'MiXeD'", &columns).unwrap(),
            "SELECT 'MiXeD'"
        );
    }
}
