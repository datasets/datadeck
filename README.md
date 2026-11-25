# DataDeck

DataDeck is a native GPUI desktop viewer for CSV, TSV, Parquet and Excel. Open a local file or paste its HTTP(S) download URL, inspect a distribution beside the table, and query the open dataset. Choose **Explore the field notes example** on the opening screen to try it with a small fictional survey.

## Run on macOS

Install Rust and Xcode, then run from the repository root:

```sh
cargo run --locked
```

To build a Finder application, run `scripts/package-macos.sh` and open `target/release/DataDeck.app`. For a development bundle, use `scripts/package-macos.sh --dev`. You can also open a dataset directly:

```sh
cargo run --locked -- examples/field-notes.csv
cargo run --locked -- https://datahub.io/core/co2-ppm/_r/-/data/co2-annmean-mlo.csv
```

![DataDeck with a column profile and query controls](changelog/images/2026-11-25-datadeck.png)

## Open and inspect

Drop a file or directory on the window, choose **Open file** (⌘O) or **Open folder** (⇧⌘O), or paste a direct HTTP(S) download link into **Open URL** (⌘L). DataHub resource links work directly. The planned dataset-page browser and publishing flow are described in [DataHub integration](docs/datahub.md).

**CSV and TSV.** DataDeck samples the opening records to guess comma, semicolon, tab or pipe delimiters and whether the first record contains headers. Quoted multiline fields, UTF-8 BOMs and Excel-style `sep=` directives are supported. Short rows have empty trailing cells. Long rows add named columns even when they appear late in a file. The status bar indicates extra fields. Click **CSV settings** to correct the delimiter or header guess, or choose **Detect again**. For ambiguous all-text data, check the header setting.

**Excel.** Each worksheet has a named tab above the grid. Click a tab to inspect or query that sheet. Its first row supplies the headers. Scroll horizontally through the tabs in a workbook with many sheets.

**Folders and Data Packages.** Open a folder, Git checkout, or a `datapackage.json`, `datapackage.yml` or `datapackage.yaml` file. A descriptor supplies the resource list in package order and can reference local files or HTTP(S) downloads. Without one, DataDeck searches for supported tables and skips hidden, dependency and build directories. Click the folder and resource name above the grid to switch tables. Each file in a multipart resource is available individually.

**Live files.** Local saves and atomic replacements refresh the selected file after writes settle. The app swaps in the new data after a successful read and reruns a visible query. The selection follows the column name and row position. The query editor retains unsent text. A failed refresh leaves the last successful view available. Live monitoring applies to the selected local file. Reopen the folder to discover newly added resources.

Click a column header to inspect its values. Numeric columns show a histogram and range. Text columns show their most frequent values. The inspector shows distinct and empty counts and identifies its sample, up to the first 1,024 rows. An ordered file can therefore have a different full-file distribution. Use SQL for exact totals across all rows. Select a cell to inspect or copy its contents. Use the arrow keys, Page Up/Down, Home or End to navigate and **I** to toggle the inspector.

Use **−** and **+** next to the text size in the status bar to adjust the table and inspector between 9 and 20 px. The default is 11 px. The **View** menu has the same controls and a reset command (⌘0). Click **Editorial** in the status bar, or use **View**, to switch between colored data marks and Monochrome. DataDeck remembers both settings.

## Query and repeat

Open **Query** (⌘K) and press Enter to run a single SQLite query against the table `data`. The first query builds a temporary local database from the loaded rows. Subsequent queries reuse it. Switch between **Source** and **Result** to compare the two. The desktop previews up to 10,000 result rows and reports the total.

For the bundled example, try `how many rows per habitat?`, `which habitat is most common?`, or `average temp_c by habitat`. These questions use a local grammar and turn into editable SQL in the same field. For other questions, enter SQL directly or paste a query written by an AI assistant.

To clean text and keep only the columns you need, run:

```sql
SELECT trim(station) AS station, survey_date, individuals
FROM data
ORDER BY survey_date, station
```

Use **Save last query…** to freeze the successful query into a script. A SQL comment records the selected sheet and CSV import settings for replay. Replay it on a second file with matching column names and redirect its full result to a new CSV:

```sh
cargo run --locked -- run recipe.sql next-survey.csv > cleaned.csv
cargo run --locked -- run recipe.sql workbook.xlsx --sheet Observations > cleaned-sheet.csv
```

The source viewer retains the displayed strings in memory. SQL projections preserve cell spelling, including padded identifiers, trailing zeros and high-precision decimals. Empty cells become NULL. Canonically spelled integers and decimals become numbers when they round-trip through SQLite’s numeric types. Numeric prompts also recognize decimal and exponent text. Arithmetic uses SQLite integer or floating-point precision. Use double quotes around identifiers with spaces. Excel uses cached cell values. CSV streams into the viewer in batches. Remote Excel and Parquet downloads finish before decoding starts.

To override a recipe’s import settings, add `--delimiter comma|tab|semicolon|pipe` or `--header yes|no`.

## Development

```sh
cargo test --locked
cargo fmt --check
```

To reproduce the warm-file parser measurement on a generated million-row CSV, run `cargo test --release million_row_csv -- --ignored --nocapture`. This measures delivery to the row callback.

See the [design rationale](DESIGN.md) for the viewer, inspector, query and recipe decisions. Project work is tracked in the **Ship a fast native tabular data viewer** Beads issue (`datadeck-pxw`).
