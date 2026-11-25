use std::fmt::Display;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use calamine::{Data, Reader, SheetType};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use ureq::ResponseExt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CsvOptions {
    pub delimiter: Option<u8>,
    pub header: Option<bool>,
}

#[derive(Debug)]
pub enum Update {
    CsvFormat {
        delimiter: u8,
        header: bool,
    },
    /// Names of additional columns, appended to the current schema.
    Widen(Vec<String>),
    Schema {
        columns: Vec<String>,
        sheets: Vec<String>,
        sheet: Option<String>,
    },
    Rows(Vec<Vec<String>>),
}

fn message(error: impl Display) -> String {
    error.to_string()
}

enum Format {
    Csv,
    Excel,
    Parquet,
}

fn format(extension: &str, prefix: &[u8]) -> Result<Format, String> {
    if prefix.starts_with(b"PAR1") {
        return Ok(Format::Parquet);
    }
    if prefix.starts_with(b"PK\x03\x04") || prefix.starts_with(b"\xd0\xcf\x11\xe0") {
        return Ok(Format::Excel);
    }
    match extension.to_ascii_lowercase().as_str() {
        "xls" | "xlsx" => Ok(Format::Excel),
        "parquet" => Ok(Format::Parquet),
        "csv" | "tsv" | "" => Ok(Format::Csv),
        other => Err(format!(
            "Unsupported file extension .{other}. Choose CSV, Excel, or Parquet."
        )),
    }
}

fn extension(name: &str) -> &str {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
}

#[cfg(test)]
fn load(source: &str, sheet: Option<&str>, emit: impl FnMut(Update) -> bool) -> Result<(), String> {
    load_with_options(source, sheet, CsvOptions::default(), emit)
}

pub fn load_with_options(
    source: &str,
    sheet: Option<&str>,
    options: CsvOptions,
    mut emit: impl FnMut(Update) -> bool,
) -> Result<(), String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        let mut response = ureq::get(source).call().map_err(message)?;
        let mime = response
            .body()
            .mime_type()
            .unwrap_or("")
            .to_ascii_lowercase();
        if mime == "text/html" {
            return Err("This URL returned an HTML page. Open the dataset’s CSV, Excel, or Parquet download URL.".into());
        }
        let ext = match mime.as_str() {
            "text/csv" => "csv".to_owned(),
            "text/tab-separated-values" => "tsv".to_owned(),
            _ => extension(response.get_uri().path()).to_owned(),
        };
        let mut stream = response.body_mut().as_reader();
        let mut prefix = Vec::with_capacity(4);
        stream
            .by_ref()
            .take(4)
            .read_to_end(&mut prefix)
            .map_err(message)?;
        let kind = format(&ext, &prefix)?;
        let mut reader = io::Cursor::new(prefix).chain(stream);
        return match kind {
            Format::Csv => load_csv_with_options(reader, options, &mut emit),
            kind => {
                // Workbooks and Parquet require seeking. Stage remote binaries on disk.
                let mut file = tempfile::NamedTempFile::new().map_err(message)?;
                io::copy(&mut reader, &mut file).map_err(message)?;
                load_binary(kind, file.path(), sheet, &mut emit)
            }
        };
    }

    let file = File::open(source).map_err(|error| format!("{source}: {error}"))?;
    let mut reader = BufReader::new(file);
    match format(extension(source), reader.fill_buf().map_err(message)?)? {
        Format::Csv => load_csv_with_options(reader, options, &mut emit),
        kind => load_binary(kind, Path::new(source), sheet, &mut emit),
    }
}

fn load_binary(
    kind: Format,
    path: &Path,
    sheet: Option<&str>,
    emit: &mut impl FnMut(Update) -> bool,
) -> Result<(), String> {
    match kind {
        Format::Excel => load_workbook(path, sheet, emit),
        Format::Parquet => load_parquet(path, emit),
        Format::Csv => unreachable!("CSV is read directly from its stream"),
    }
}

fn send_rows(
    rows: impl Iterator<Item = Result<Vec<String>, String>>,
    emit: &mut impl FnMut(Update) -> bool,
) -> Result<(), String> {
    let mut limit = 64;
    let mut batch = Vec::with_capacity(limit);
    for row in rows {
        batch.push(row?);
        if batch.len() == limit {
            if !emit(Update::Rows(batch)) {
                return Ok(());
            }
            limit = 2048;
            batch = Vec::with_capacity(limit);
        }
    }
    if !batch.is_empty() {
        emit(Update::Rows(batch));
    }
    Ok(())
}

const SEPARATORS: [u8; 4] = [b',', b'\t', b';', b'|'];

fn csv_reader<R: Read>(reader: R, delimiter: u8) -> csv::Reader<R> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(reader)
}

// Compare actual parsed records so punctuation inside quoted fields does not count.
fn infer_delimiter(sample: &[u8]) -> u8 {
    let mut best = (0, 0, 0);
    let mut delimiter = b',';
    for candidate in SEPARATORS {
        let mut widths = std::collections::BTreeMap::new();
        let mut multi_field = 0;
        let mut total = 0;
        for record in csv_reader(sample, candidate)
            .byte_records()
            .take(32)
            .flatten()
        {
            total += 1;
            if record.len() > 1 {
                *widths.entry(record.len()).or_insert(0usize) += 1;
                multi_field += 1;
            }
        }
        if total == 0 {
            continue;
        }
        let (matching, width) = widths
            .into_iter()
            .map(|(width, n)| (n, width))
            .max()
            .unwrap_or_default();
        let score = (matching * 100 / total, multi_field * 100 / total, width);
        if score > best {
            best = score;
            delimiter = candidate;
        }
    }
    delimiter
}

fn infer_header(first: &csv::StringRecord, next: &[csv::StringRecord]) -> bool {
    // A number, boolean or date in the first record is strong evidence of a data row.
    // All-text input defaults to a header unless a label recurs in the same column.
    !first.iter().enumerate().any(|(index, field)| {
        let value = field.trim();
        let date = value.split(['T', ' ']).next().unwrap_or(value);
        let mut parts = date.split(['-', '/']);
        let looks_like_date = matches!((parts.next(), parts.next(), parts.next(), parts.next()),
            (Some(a),Some(b),Some(c),None) if (a.len() == 4 || c.len() == 4)
            && [a,b,c].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())));
        !value.is_empty() && (value.parse::<f64>().is_ok()
            || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
            || looks_like_date || next.iter().any(|row| row.get(index) == Some(field)))
    })
}

fn directive(sample: &[u8]) -> Option<(u8, usize)> {
    let end = sample
        .iter()
        .position(|b| matches!(b, b'\r' | b'\n'))
        .unwrap_or(sample.len());
    let line = &sample[..end];
    if line.len() == 5 && line[..4].eq_ignore_ascii_case(b"sep=") && SEPARATORS.contains(&line[4]) {
        let mut start = end;
        while start < sample.len() && matches!(sample[start], b'\r' | b'\n') {
            start += 1;
        }
        return Some((line[4], start));
    }
    None
}

pub fn load_csv_with_options(
    reader: impl Read,
    options: CsvOptions,
    mut emit: impl FnMut(Update) -> bool,
) -> Result<(), String> {
    // Bound the lookahead even for one extremely wide record. Replay exactly those bytes.
    let mut reader = BufReader::new(reader);
    let mut sample = Vec::new();
    {
        let mut prefix = (&mut reader).take(32 * 1024);
        for _ in 0..32 {
            if prefix.read_until(b'\n', &mut sample).map_err(message)? == 0 {
                break;
            }
        }
    }
    let sample = sample.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&sample);
    let separator_line = directive(sample);
    let sample = &sample[separator_line.map_or(0, |(_, offset)| offset)..];
    let delimiter = options
        .delimiter
        .or(separator_line.map(|(separator, _)| separator))
        .unwrap_or_else(|| infer_delimiter(sample));
    if !SEPARATORS.contains(&delimiter) {
        return Err("Choose comma, tab, semicolon, or pipe as separator.".into());
    }
    let mut csv = csv_reader(sample.chain(reader), delimiter);
    let mut first = csv::StringRecord::new();
    if !csv.read_record(&mut first).map_err(message)? {
        return Err("The CSV is empty.".into());
    }
    let header = options.header.unwrap_or_else(|| {
        let probes: Vec<_> = csv_reader(sample, delimiter)
            .records()
            .skip(1)
            .take(16)
            .map_while(Result::ok)
            .collect();
        infer_header(&first, &probes)
    });
    let mut width = first.len();
    let names = |start: usize, end: usize| {
        (start..end)
            .map(|i| format!("column_{}", i + 1))
            .collect::<Vec<_>>()
    };
    let columns = if header {
        first.iter().map(str::to_owned).collect()
    } else {
        names(0, width)
    };
    if !emit(Update::CsvFormat { delimiter, header })
        || !emit(Update::Schema {
            columns,
            sheets: vec![],
            sheet: None,
        })
    {
        return Ok(());
    }
    let mut limit = 64;
    let mut batch = Vec::with_capacity(limit);
    if !header {
        batch.push(first.iter().map(str::to_owned).collect::<Vec<_>>());
    }
    let mut record = csv::StringRecord::new();
    while csv.read_record(&mut record).map_err(message)? {
        if record.len() > width {
            if !emit(Update::Widen(names(width, record.len()))) {
                return Ok(());
            }
            width = record.len();
            for row in &mut batch {
                row.resize(width, String::new());
            }
        }
        let mut row: Vec<_> = record.iter().map(str::to_owned).collect();
        row.resize(width, String::new());
        batch.push(row);
        if batch.len() == limit {
            if !emit(Update::Rows(batch)) {
                return Ok(());
            }
            limit = 2048;
            batch = Vec::with_capacity(limit);
        }
    }
    if !batch.is_empty() {
        emit(Update::Rows(batch));
    }
    Ok(())
}

fn load_parquet(path: &Path, emit: &mut impl FnMut(Update) -> bool) -> Result<(), String> {
    let reader = SerializedFileReader::new(File::open(path).map_err(message)?).map_err(message)?;
    let schema = reader.metadata().file_metadata().schema();
    let columns = schema
        .get_fields()
        .iter()
        .map(|field| field.name().to_owned())
        .collect();
    if !emit(Update::Schema {
        columns,
        sheets: vec![],
        sheet: None,
    }) {
        return Ok(());
    }
    send_rows(
        reader.get_row_iter(None).map_err(message)?.map(|row| {
            Ok(row
                .map_err(message)?
                .into_columns()
                .into_iter()
                .map(|(_, value)| match value {
                    Field::Null => String::new(),
                    Field::Str(text) => text,
                    Field::Float(value) => value.to_string(),
                    Field::Double(value) => value.to_string(),
                    other => other.to_string(),
                })
                .collect())
        }),
        emit,
    )
}

fn cell_text(cell: &Data) -> String {
    if let Data::DateTime(value) = cell {
        if value.is_duration() {
            if let Some(duration) = value.as_duration() {
                return duration.to_string();
            }
        } else if let Some(datetime) = value.as_datetime() {
            return datetime.to_string();
        }
    }
    cell.to_string()
}

fn load_workbook(
    path: &Path,
    selected: Option<&str>,
    emit: &mut impl FnMut(Update) -> bool,
) -> Result<(), String> {
    let mut workbook = calamine::open_workbook_auto(path).map_err(message)?;
    let sheets: Vec<String> = workbook
        .sheets_metadata()
        .iter()
        .filter(|sheet| sheet.typ == SheetType::WorkSheet)
        .map(|sheet| sheet.name.clone())
        .collect();
    let sheet = selected
        .or_else(|| sheets.first().map(String::as_str))
        .ok_or_else(|| "Workbook has no worksheets.".to_owned())?
        .to_owned();
    let range = workbook.worksheet_range(&sheet).map_err(message)?;
    let mut rows = range.rows();
    let columns = rows
        .next()
        .unwrap_or_default()
        .iter()
        .map(cell_text)
        .collect();
    if !emit(Update::Schema {
        columns,
        sheets,
        sheet: Some(sheet),
    }) {
        return Ok(());
    }
    send_rows(
        rows.map(|row| Ok(row.iter().map(cell_text).collect())),
        emit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufWriter, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    fn collect(source: &str, sheet: Option<&str>) -> Result<Vec<Update>, String> {
        let mut updates = Vec::new();
        load(source, sheet, |update| {
            updates.push(update);
            true
        })?;
        Ok(updates)
    }

    fn fixture(name: &str) -> String {
        Path::new(file!())
            .parent()
            .unwrap()
            .join("../tests/fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn parse_csv(input: &[u8], options: CsvOptions) -> (Vec<String>, Vec<Vec<String>>, u8, bool) {
        let mut columns = Vec::new();
        let mut all_rows = Vec::new();
        let mut format = None;
        load_csv_with_options(input, options, |update| {
            match update {
                Update::Schema { columns: names, .. } => columns = names,
                Update::CsvFormat { delimiter, header } => format = Some((delimiter, header)),
                Update::Widen(added) => columns.extend(added),
                Update::Rows(batch) => all_rows.extend(batch),
            }
            true
        })
        .unwrap();
        let (delimiter, header) = format.unwrap();
        (columns, all_rows, delimiter, header)
    }

    #[test]
    fn detects_delimiters_and_preserves_quoted_fields() {
        for delimiter in SEPARATORS {
            let d = char::from(delimiter);
            let text = format!("\u{feff}Name{d}Count\r\n\" a,b|c;d\n \"{d}03\r\n");
            let (columns, rows, detected, header) =
                parse_csv(text.as_bytes(), CsvOptions::default());
            assert_eq!(delimiter, detected);
            assert!(header);
            assert_eq!(columns, ["Name", "Count"]);
            assert_eq!(rows, [[" a,b|c;d\n ", "03"]]);
        }
    }

    #[test]
    fn detects_headerless_records_and_honors_overrides() {
        let (columns, rows, delimiter, header) = parse_csv(
            b"2026-05-12|green\n2026-05-13|pink\n",
            CsvOptions::default(),
        );
        assert_eq!(columns, ["column_1", "column_2"]);
        assert_eq!(rows.len(), 2);
        assert_eq!((delimiter, header), (b'|', false));
        let (columns, rows, _, header) = parse_csv(
            b"Name,Alias\nAlice,Bob\n",
            CsvOptions {
                delimiter: Some(b';'),
                header: Some(false),
            },
        );
        assert_eq!(columns, ["column_1"]);
        assert_eq!(rows[0], ["Name,Alias"]);
        assert!(!header);
        let (columns, rows, d, h) = parse_csv(
            b"sep=;\r\n2020;2021\r\n5;6\r\n",
            CsvOptions {
                delimiter: None,
                header: Some(true),
            },
        );
        assert_eq!(columns, ["2020", "2021"]);
        assert_eq!(rows, [["5", "6"]]);
        assert_eq!((d, h), (b';', true));
    }

    #[test]
    fn short_rows_and_late_extra_fields_survive_preview() {
        let mut text = "A,B\n".to_owned();
        for _ in 0..70 {
            text.push_str("1,2\n");
        }
        text.push_str("short\n3,4,5,6\n");
        let (columns, rows, _, _) = parse_csv(text.as_bytes(), CsvOptions::default());
        assert_eq!(columns, ["A", "B", "column_3", "column_4"]);
        assert_eq!(rows.len(), 72);
        assert_eq!(rows[70], ["short", "", "", ""]);
        assert_eq!(rows[71], ["3", "4", "5", "6"]);
    }

    #[test]
    fn tsv_extension_and_explicit_header() {
        let mut file = tempfile::Builder::new().suffix(".tsv").tempfile().unwrap();
        write!(file, "code\tvalue\n01\t2\n").unwrap();
        let events = collect(file.path().to_str().unwrap(), None).unwrap();
        assert!(matches!(
            &events[0],
            Update::CsvFormat {
                delimiter: b'\t',
                header: true
            }
        ));
    }

    #[test]
    fn workbook_sheet_selection_and_parquet_values() {
        for name in ["sample.xls", "sample.xlsx"] {
            let updates = collect(&fixture(name), None).unwrap();
            assert!(
                matches!(&updates[..], [Update::Schema { columns, sheets, sheet }, Update::Rows(rows)] if columns == &["Name","Count"] && sheets == &["Sales","Notes"] && sheet.as_deref() == Some("Sales") && rows[0] == [" A ","12"])
            );
            let updates = collect(&fixture(name), Some("Notes")).unwrap();
            assert!(
                matches!(&updates[..], [Update::Schema { sheet, .. }, Update::Rows(rows)] if sheet.as_deref() == Some("Notes") && rows[0] == ["Hello"])
            );
            assert!(collect(&fixture(name), Some("Missing")).is_err());
        }
        let updates = collect(&fixture("sample.parquet"), None).unwrap();
        assert!(
            matches!(&updates[..], [Update::Schema { columns, .. }, Update::Rows(rows)] if columns == &["Name","Count"] && rows[0] == [" A ","12"] && rows[1] == ["B",""])
        );
    }

    fn serve(path: &str, mime: &'static str, bytes: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/{path}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = BufReader::new(socket.try_clone().unwrap());
            loop {
                let mut header = String::new();
                if request.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                    break;
                }
            }
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
            socket.write_all(&bytes).unwrap();
        });
        url
    }

    #[test]
    fn reads_csv_and_binary_downloads_over_http() {
        let url = serve("data.csv?download=1", "text/csv", b"a,b\n1,2\n".to_vec());
        let updates = collect(&url, None).unwrap();
        assert!(
            matches!(&updates[..], [Update::CsvFormat { delimiter: b',', header: true }, Update::Schema { .. }, Update::Rows(rows)] if rows[0] == ["1","2"])
        );

        for name in ["sample.xlsx", "sample.parquet"] {
            let url = serve(
                "download",
                "application/octet-stream",
                std::fs::read(fixture(name)).unwrap(),
            );
            let updates = collect(&url, None).unwrap();
            assert!(
                matches!(&updates[..], [Update::Schema { .. }, Update::Rows(rows)] if rows[0] == [" A ","12"])
            );
        }

        let url = serve("dataset", "text/html", b"<html></html>".to_vec());
        assert!(collect(&url, None).unwrap_err().contains("download URL"));
    }

    #[test]
    fn first_preview_can_cancel_a_large_load() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "A,B").unwrap();
        for _ in 0..100 {
            writeln!(file, "1,2").unwrap();
        }
        writeln!(file, "invalid").unwrap();
        let mut sizes = Vec::new();
        load(file.path().to_str().unwrap(), None, |update| {
            if let Update::Rows(rows) = update {
                sizes.push(rows.len());
                return false;
            }
            true
        })
        .unwrap();
        assert_eq!(sizes, [64]);
    }

    #[test]
    #[ignore = "local parsing benchmark; run with --ignored --nocapture"]
    fn million_row_csv() {
        let file = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        let mut writer = BufWriter::new(file.as_file());
        writeln!(writer, "id,region,units,description").unwrap();
        for i in 0..1_000_000 {
            writeln!(writer, "{i},Europe,{},  sample {}  ", i % 100, i % 10).unwrap();
        }
        writer.flush().unwrap();
        let start = Instant::now();
        let mut preview = None;
        let mut count = 0;
        load(file.path().to_str().unwrap(), None, |update| {
            if let Update::Rows(rows) = update {
                preview.get_or_insert_with(|| start.elapsed());
                count += rows.len();
            }
            true
        })
        .unwrap();
        assert_eq!(count, 1_000_000);
        eprintln!(
            "{} bytes, 1 million rows, 4 columns. First batch {:?}, all rows {:?}",
            file.as_file().metadata().unwrap().len(),
            preview.unwrap(),
            start.elapsed()
        );
    }
}
