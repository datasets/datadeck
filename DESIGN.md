# DataDeck design

## Reading a dataset

Give the table most of the window. Use IBM Plex Mono for field names, values, row positions and counts. Use the platform sans for actions and explanations. Right-align numeric values so a change in magnitude is visible down a column. White surfaces, black type and light grey rules carry the resting view. The default table uses 11 px type in 24 px rows. A darker header rule, pale column tint and directly labelled chart marks connect the selected field to its profile. Keep the filename and commands in a single bar to leave more room for data. The font pairing draws on Zed's separation of interface and editor typography and IBM's related Plex families. Tad provides a reference for a desktop viewer centered on one open table. [Zed typography](https://zed.dev/docs/visual-customization) · [IBM Plex](https://www.ibm.com/design/language/typography/typeface/) · [Tad](https://www.tadviewer.com/)

Editorial uses restrained raspberry, sky, lilac and ochre for chart marks. Monochrome uses the same shapes and labels in greys. These choices draw on the close-set type, labelled amounts and white gutters in *Billion-Dollar-O-Gram* in David McCandless’s 2013 revised edition of *Information Is Beautiful*. The chart labels sit beside the marks they describe. The selected column name provides the heading and the observed values provide its scale. This follows the direct labelling and detail-on-demand approach of Information Is Beautiful. [Billion Dollar-o-Gram](https://informationisbeautiful.net/visualizations/the-billion-dollar-gram/) · [Mountains Out of Molehills](https://informationisbeautiful.net/visualizations/mountains-out-of-molehills/)

## Column summaries

A histogram answers “where are the values concentrated?” Frequency bars answer “which labels dominate?” Both belong next to the selected column so a reader can compare them with the underlying rows. Bound the initial calculation to the first 1,024 rows, label its coverage, and provide exact full-data aggregation through SQL. This makes switching columns interactive while a large file is arriving. [Positron Data Explorer](https://positron.posit.co/data-explorer-summary-panel.html) · [MotherDuck Column Explorer](https://motherduck.com/blog/introducing-column-explorer/)

## SQL

Open the query field on demand. Build the database lazily, on a worker, so a user who only wants to view a file pays no SQL import cost. Queries always refer to the full open source or selected sheet, including when viewing a previous result. Preserve the source grid and show a distinct result view. Apply a preview limit on the desktop and stream the complete result from the CLI.

## Plain-language questions

Support quick, predictable questions about row counts, common categories and numeric aggregates. Bind column names against the actual schema and display the translated SQL in the editor. A question with an ambiguous column needs a correction before execution. This path runs locally. An external AI can supply arbitrary SQLite SELECT queries through the same interface.

## Repeatable wrangling

Use SQL projection for choosing columns, functions such as trim for cleaning text, and ORDER BY for repeatable ordering. Save the last successful query as a plain SQL file. The desktop and CLI use the same importer, type conversion and execution code. This gives the user small reusable primitives while keeping the main window focused on inspection.

## Bookmark Library

A page about a dataset raises two different questions. “Why did I save this?” needs a title, a source and room for a personal note. “What’s in the file?” needs a grid and a profile. The Library keeps the first question one click from the viewer and offers linked files as the path to the second. Dropping a link saves it immediately, which also works for sites that block previews. Switching surfaces retains the open table, query and selection. Publication to DataHub would be a separate, deliberate action on a saved collection.
