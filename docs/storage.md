# Where DataDeck keeps your data

## The capture inbox

DataDeck keeps one Markdown file per captured web link in `~/.config/datadeck/inbox/`. If `XDG_CONFIG_HOME` points to an absolute directory, the inbox is `$XDG_CONFIG_HOME/datadeck/inbox/`. Run `datadeck --inbox-path` to print the path, or choose **Open inbox folder ↗** in the Library. For a single capture, choose **Details → Open Markdown file ↗**. You can copy the inbox, including its `assets/` directory, to back it up or work with it on another computer.

To add a capture from another tool, create a UTF-8 file such as `climate-notes.md` in that directory:

```markdown
---
url: https://ourworldindata.org/co2-emissions
title: Carbon dioxide emissions
---

Compare the change in per-capita emissions with the change in total emissions.

Check which countries have a continuous historical series.
```

The only required frontmatter field is `url`. DataDeck adds metadata as it becomes available. The note starts below the closing `---` and can contain regular Markdown, including headings, links, paragraphs, and code blocks. Edit the body in your favorite text editor and return to the Library to read it. A new capture or a repeat capture determines the position in the log. Editing an existing note keeps its place. For files created by another tool, supply `captured_at` if you want to control their position. Entries without a date follow the dated captures.

## Frontmatter fields

| Field | What other tools can use it for |
| --- | --- |
| `url` | Original webpage or dataset address. |
| `title` | Display title, whether retrieved from the page or edited by you. |
| `kind` | Category of capture, such as `dataset`, `story` or `visualization`. |
| `captured_at` | Timestamp of the most recent capture in RFC 3339 format. |
| `files` | Data resources that can be opened in the viewer. |
| `images`, `visualizations` | Discovered links with a title and URL. |
| `thumbnail` | Relative path to a cached image under `assets/`. |
| `datadeck` | Bookkeeping for app-created files, including stable identification and edit tracking. Other readers can ignore it. |

Custom frontmatter fields, such as `tags` or `project`, survive app edits. Saving through DataDeck can normalize YAML quoting and formatting and remove YAML comments. Store annotations you want to keep in a field or in the Markdown body. An unreadable capture file is left in place while the Library loads the other entries.

## Read captures from another tool

To find a reference across every saved title, link and note, search the folder directly:

```sh
rg -i 'emissions' "$(datadeck --inbox-path)" --glob '*.md'
```

This Python example uses PyYAML (`python3 -m pip install PyYAML` in your Python environment). It prints the newest dated captures first, with the title, original URL and first line of notes. The split stops at the end of the frontmatter so a horizontal rule inside a note is preserved.

```python
from pathlib import Path
import os

import yaml

config = Path(os.environ.get("XDG_CONFIG_HOME", ""))
if not config.is_absolute():
    config = Path.home() / ".config"
inbox = config / "datadeck" / "inbox"
entries = []
for file in inbox.glob("*.md"):
    text = file.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        continue
    header, separator, note = text[4:].partition("\n---\n")
    if separator:
        meta = yaml.safe_load(header) or {}
        first_line = next(iter(note.strip().splitlines()), "")
        entries.append((str(meta.get("captured_at") or ""), meta.get("title") or file.stem, meta.get("url", ""), first_line))

for captured_at, title, url, first_line in sorted(entries, reverse=True):
    print(captured_at, title, url, first_line, sep="\t")
```

## Existing installations

An upgrade imports the earlier bookmark library from `~/Library/Application Support/DataDeck/library.sqlite3` on macOS. DataDeck reads a consistent snapshot, leaves the original database in place and records the completed migration. If a Markdown capture already has the same URL and a different note, the Markdown file wins and the differing legacy note is preserved under `inbox/Recovered Notes/`. Continue capturing in the new version after the import. If the old version saved conflicting drafts, its `Recovered Notes/` folder remains alongside that database.

## Opened tables and SQL

CSV, Parquet, Excel, pasted spreadsheet cells, and query results are separate from the capture inbox. An opened file stays in its original location. The viewer holds decoded rows in memory and builds a private temporary SQLite database for SQL queries. Closing the dataset releases that database. **Save last query…** writes a SQL recipe to the location you pick. The command-line runner writes CSV to standard output, which you can redirect to a file.
