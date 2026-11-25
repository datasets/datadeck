use super::Rows;

#[derive(Clone, Copy)]
pub(super) struct Cell {
    pub row: usize,
    pub column: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Rectangle {
    pub top: usize,
    pub bottom: usize,
    pub left: usize,
    pub right: usize,
}

impl Rectangle {
    pub fn between(anchor: Cell, cursor: Cell) -> Self {
        Self {
            top: anchor.row.min(cursor.row),
            bottom: anchor.row.max(cursor.row),
            left: anchor.column.min(cursor.column),
            right: anchor.column.max(cursor.column),
        }
    }

    pub fn single(self) -> bool {
        self.top == self.bottom && self.left == self.right
    }

    pub fn contains(self, row: usize, column: usize) -> bool {
        (self.top..=self.bottom).contains(&row) && (self.left..=self.right).contains(&column)
    }

    pub fn text(self, rows: &Rows) -> String {
        if self.single() {
            return rows[self.top].get(self.left).cloned().unwrap_or_default();
        }
        let mut writer = csv::WriterBuilder::new()
            .delimiter(b'\t')
            .terminator(csv::Terminator::CRLF)
            .from_writer(Vec::new());
        for i in self.top..=self.bottom {
            let row = &rows[i];
            let fields = (self.left..=self.right)
                .map(|j| row.get(j).map(String::as_str).unwrap_or_default());
            writer
                .write_record(fields)
                .expect("fixed-width record in a memory buffer");
        }
        let mut bytes = writer.into_inner().expect("flush to memory");
        bytes.truncate(bytes.len() - 2); // Omit the final CRLF when copying to the clipboard.
        String::from_utf8(bytes).expect("TSV written from UTF-8 strings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_rectangle_round_trips_tabs_quotes_newlines_and_short_rows() {
        let mut rows = Rows::default();
        rows.append(vec![vec!["skip".into(), "a\tb".into(), "\"q\"".into()]].into());
        rows.append(vec![vec!["skip".into(), "a\r\nb".into()], vec![]].into());
        let rect = Rectangle::between(Cell { row: 2, column: 2 }, Cell { row: 0, column: 1 });
        assert!(rect.contains(1, 1));
        assert!(!rect.contains(0, 0));
        let text = rect.text(&rows);
        assert_eq!(text, "\"a\tb\"\t\"\"\"q\"\"\"\r\n\"a\r\nb\"\t\r\n\t");
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(b'\t')
            .has_headers(false)
            .from_reader(text.as_bytes());
        let parsed: Vec<Vec<String>> = reader
            .records()
            .map(|r| r.unwrap().iter().map(str::to_owned).collect())
            .collect();
        assert_eq!(parsed, [["a\tb", "\"q\""], ["a\r\nb", ""], ["", ""]]);
    }

    #[test]
    fn copies_one_cell_verbatim_and_keeps_blank_rows_in_single_column_selections() {
        let mut rows = Rows::default();
        rows.append(vec![vec!["a\tb\"\n".into()], vec![], vec!["c".into()]].into());
        let first = Cell { row: 0, column: 0 };
        assert_eq!(Rectangle::between(first, first).text(&rows), "a\tb\"\n");
        let bottom = Cell { row: 2, column: 0 };
        assert_eq!(
            Rectangle::between(first, bottom).text(&rows),
            "\"a\tb\"\"\n\"\r\n\"\"\r\nc"
        );
    }
}
