use std::sync::LazyLock;

use regex::Regex;
use url::Url;

#[derive(Debug, PartialEq)]
pub struct Capture {
    pub url: String,
    pub note: String,
}

// Prefer explicit URLs. A short list of common suffixes also recognizes domain
// names in dictated text without treating filenames such as survey.csv as sites.
static LINKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r#"(?ix)
    \b(https?:// | www\.)[^\s<>"“”‘’]+
    | \b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+(?:com|org|net|edu|gov|io|ai|co|uk|de|fr|dev|app|info|eu)\b(?:[/?\#][^\s<>"“”‘’]*)?
    "#
).expect("valid capture URL pattern")
});

pub fn capture_text(text: &str) -> Option<Capture> {
    if let Some(url) = crate::web_links::web_url(text)
        && url.username().is_empty()
        && url.password().is_none()
    {
        return Some(Capture {
            url: url.into(),
            note: String::new(),
        });
    }
    for found in LINKS.find_iter(text) {
        let before = &text[..found.start()];
        if before.ends_with(['@', '/']) {
            continue;
        }
        let mut link = found
            .as_str()
            .trim_end_matches(['.', ',', ';', '!', ':', '\'']);
        while let Some((_, close)) =
            [('(', ')'), ('[', ']'), ('{', '}')]
                .into_iter()
                .find(|(open, close)| {
                    link.ends_with(*close)
                        && link.matches(*close).count() > link.matches(*open).count()
                })
        {
            link = &link[..link.len() - close.len_utf8()];
        }
        let qualified = if link.contains("://") {
            link.to_owned()
        } else {
            format!("https://{link}")
        };
        let Ok(url) = Url::parse(&qualified) else {
            continue;
        };
        if !url.has_host() || !url.username().is_empty() || url.password().is_some() {
            continue;
        }
        let after = &text[found.start() + link.len()..];
        let (left, right) = trim_wrappers(before, after);
        let (left, right) = (left.trim(), right.trim());
        let note = match (left.is_empty(), right.is_empty()) {
            (true, _) => right.to_owned(),
            (_, true) => left.to_owned(),
            _ => format!(
                "{left}{}{right}",
                if before.ends_with('\n') || after.starts_with('\n') {
                    "\n"
                } else {
                    " "
                }
            ),
        };
        return Some(Capture {
            url: url.into(),
            note,
        });
    }
    None
}

// A paste or committed dictation replaces the active selection. When somebody
// has dictated a comment and then pastes a complete URL at its edge, keep a
// word boundary so the comment and address can be recognized independently.
pub fn insert_capture(
    current: &str,
    selection: std::ops::Range<usize>,
    value: &str,
) -> Option<String> {
    let prefix = &current[..selection.start];
    let suffix = &current[selection.end..];
    let separate = capture_text(value).is_some();
    let separator = |left: &str, right: &str| {
        if separate
            && left.chars().last().is_some_and(|c| !c.is_whitespace())
            && right.chars().next().is_some_and(|c| !c.is_whitespace())
        {
            " "
        } else {
            ""
        }
    };
    let combined = format!(
        "{prefix}{}{value}{}{suffix}",
        separator(prefix, value),
        separator(value, suffix)
    );
    capture_text(&combined).map(|_| combined)
}

fn trim_wrappers<'a>(before: &'a str, after: &'a str) -> (&'a str, &'a str) {
    let mut left = before.trim_end();
    let mut right = after.trim_start();
    for (open, close) in [
        ('<', '>'),
        ('(', ')'),
        ('[', ']'),
        ('"', '"'),
        ('“', '”'),
        ('‘', '’'),
        ('\'', '\''),
    ] {
        if let (Some(start), Some(end)) = (left.strip_suffix(open), right.strip_prefix(close)) {
            left = start;
            right = end;
            break;
        }
    }
    // Standard Markdown links keep the readable label in the note.
    if before.ends_with("](")
        && after.starts_with(')')
        && let Some(pos) = left.rfind('[')
        && pos == 0
    {
        left = left.trim_start_matches('[').trim_end_matches(']');
    }
    (
        left.trim_end_matches([' ', '—', '–']),
        right.trim_start_matches([' ', '.', ',', ':', ';', '!', '—', '–']),
    )
}

// Outside the Library, preserve normal spreadsheet paste even when a table
// includes web addresses in one of its fields.
pub fn clipboard_table(text: &str) -> bool {
    if text.contains('\t') {
        return true;
    }
    if !text.trim().contains('\n') {
        return false;
    }
    [b',', b';', b'|'].into_iter().any(|delimiter| {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(delimiter)
            .from_reader(text.as_bytes());
        let mut records = reader.records();
        if let (Some(Ok(a)), Some(Ok(b))) = (records.next(), records.next()) {
            a.len() > 1 && a.len() == b.len()
        } else {
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_one_link_and_retains_the_note_and_query_string() {
        for (text, expected) in [
            (
                "https://example.org/plot?x=1&y=2\nCheck the 1990 peak.",
                "Check the 1990 peak.",
            ),
            (
                "Check the 1990 peak.\n<https://example.org/plot?x=1&y=2>",
                "Check the 1990 peak.",
            ),
            (
                "[Annual trend](https://example.org/plot?x=1&y=2)",
                "Annual trend",
            ),
            ("www.example.org/plot — nice chart", "nice chart"),
            (
                "Keep this datahub.io/core/co2-ppm for the report",
                "Keep this for the report",
            ),
        ] {
            let capture = capture_text(text).unwrap();
            assert_eq!(capture.note, expected);
            if text.contains("x=1") {
                assert_eq!(capture.url, "https://example.org/plot?x=1&y=2");
            }
        }
        let item =
            capture_text("https://example.org/Page_(chart). Another source https://other.org")
                .unwrap();
        assert_eq!(item.url, "https://example.org/Page_(chart)");
        assert_eq!(item.note, "Another source https://other.org");
        assert!(capture_text("contact editor@example.org").is_none());
        assert!(capture_text("file:///tmp/example.org").is_none());
        assert!(capture_text("keep survey.csv").is_none());
        assert_eq!(
            capture_text("https://example.org?q=a!").unwrap().url,
            "https://example.org/?q=a!"
        );
    }

    #[test]
    fn comment_first_paste_keeps_the_existing_draft_and_replaces_only_the_selection() {
        let draft = "Compare with last month";
        let raw = insert_capture(draft, draft.len()..draft.len(), "https://example.org").unwrap();
        assert_eq!(capture_text(&raw).unwrap().note, draft);
        let raw = insert_capture("Read this again", 5..9, "https://example.org").unwrap();
        assert_eq!(capture_text(&raw).unwrap().note, "Read again");
        assert!(insert_capture("", 0..0, "Compare two charts").is_none());
    }

    #[test]
    fn detects_clipboard_tables_with_urls_before_free_text_capture() {
        assert!(clipboard_table("item,url\nA,https://example.org"));
        assert!(clipboard_table("item\thttps://example.org"));
        assert!(!clipboard_table(
            "https://example.org\nLook at 2008, then 2020."
        ));
    }
}
