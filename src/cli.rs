use std::{cell::RefCell, fs, io};

use crate::{data, query};

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ImportSettings {
    delimiter: Option<u8>,
    header: Option<bool>,
    sheet: Option<String>,
}

const IMPORT_PREFIX: &str = "-- datadeck-import ";

pub fn freeze(sql: &str, options: data::CsvOptions, sheet: Option<&str>) -> String {
    let settings = ImportSettings {
        delimiter: options.delimiter,
        header: options.header,
        sheet: sheet.map(str::to_owned),
    };
    format!(
        "{IMPORT_PREFIX}{}\n{sql}",
        serde_json::to_string(&settings).expect("serialize simple settings")
    )
}

pub fn replay(args: &[String]) -> Result<(), String> {
    replay_to(args, io::stdout().lock())
}

fn replay_to(args: &[String], output: impl io::Write) -> Result<(), String> {
    let [recipe, source, flags @ ..] = args else {
        return Err("Usage: datadeck run RECIPE.sql INPUT [--sheet SHEET] [--delimiter comma|tab|semicolon|pipe] [--header yes|no]".into());
    };
    let sql = fs::read_to_string(recipe).map_err(|error| format!("{recipe}: {error}"))?;
    let mut settings: ImportSettings = match sql
        .lines()
        .next()
        .and_then(|s| s.strip_prefix(IMPORT_PREFIX))
    {
        Some(json) => serde_json::from_str(json)
            .map_err(|e| format!("Invalid import settings in {recipe}: {e}"))?,
        None => ImportSettings::default(),
    };
    for pair in flags.chunks(2) {
        let [key, value] = pair else {
            return Err("Expected a value after the final option.".into());
        };
        match key.as_str() {
            "--sheet" => settings.sheet = Some(value.clone()),
            "--header" => {
                settings.header = Some(match value.as_str() {
                    "yes" => true,
                    "no" => false,
                    _ => return Err("Use --header yes or no.".into()),
                })
            }
            "--delimiter" => {
                settings.delimiter = Some(match value.as_str() {
                    "comma" => b',',
                    "tab" => b'\t',
                    "semicolon" => b';',
                    "pipe" => b'|',
                    _ => return Err("Use comma, tab, semicolon, or pipe.".into()),
                })
            }
            _ => return Err(format!("Unknown replay option {key}")),
        }
    }
    let options = data::CsvOptions {
        delimiter: settings.delimiter,
        header: settings.header,
    };
    let mut names = Vec::new();
    let mut database = None;
    let mut width = 0;
    let mut import_error = None;
    data::load_with_options(source, settings.sheet.as_deref(), options, |batch| {
        let result = match batch {
            data::Update::Schema { columns, .. } => {
                width = columns.len();
                names = query::column_names(&columns);
                query::database(&columns).map(|db| database = Some(db))
            }
            data::Update::CsvFormat { .. } => Ok(()),
            data::Update::Widen(extra) => {
                names.extend(extra);
                names = query::column_names(&names);
                let result = database
                    .as_ref()
                    .ok_or_else(|| "Missing input schema.".to_owned())
                    .and_then(|db| query::add_columns(db, &names[width..]));
                width = names.len();
                result
            }
            data::Update::Rows(rows) => database
                .as_mut()
                .ok_or_else(|| "Missing input schema.".to_string())
                .and_then(|db| query::insert(db, width, &rows)),
        };
        import_error = result.err();
        import_error.is_none()
    })?;
    if let Some(error) = import_error {
        return Err(error);
    }
    let database = database.ok_or("Input did not contain a table.")?;
    let writer = RefCell::new(csv::Writer::from_writer(output));
    let write_record = |fields: Vec<String>| {
        writer
            .borrow_mut()
            .write_record(fields)
            .map_err(|err| err.to_string())
    };
    query::select(&database, &sql, write_record, write_record)?;
    writer.into_inner().flush().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_recipe_runs_on_two_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let recipe = directory.path().join("clean.sql");
        fs::write(&recipe, "SELECT trim(name) AS name FROM data ORDER BY name").unwrap();
        for (name, input, expected) in [
            ("first", "name,ignored\n  Koi ,x\n", "name\nKoi\n"),
            (
                "second",
                "name,ignored\n\" Red, Blue \",y\n",
                "name\n\"Red, Blue\"\n",
            ),
        ] {
            let path = directory.path().join(format!("{name}.csv"));
            fs::write(&path, input).unwrap();
            let mut bytes = Vec::new();
            replay_to(
                &[recipe.display().to_string(), path.display().to_string()],
                &mut bytes,
            )
            .unwrap();
            assert_eq!(String::from_utf8(bytes).unwrap(), expected);
        }
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;
    #[test]
    fn saved_dialect_replays_headerless_ragged_rows() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("events.csv");
        fs::write(&input, "alpha|beta\ngamma|delta|extra\n").unwrap();
        let recipe = dir.path().join("events.sql");
        let sql = freeze(
            "SELECT * FROM data ORDER BY rowid",
            data::CsvOptions {
                delimiter: Some(b'|'),
                header: Some(false),
            },
            None,
        );
        fs::write(&recipe, sql).unwrap();
        let mut out = vec![];
        replay_to(
            &[recipe.display().to_string(), input.display().to_string()],
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "column_1,column_2,column_3\nalpha,beta,\ngamma,delta,extra\n"
        );
    }
}
