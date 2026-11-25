//! The Markdown document is the record. Internal fingerprints distinguish an editor's
//! changes from older asynchronous results without requiring a separate database.
use super::{Bookmark, FoundLink, Kind, http_url, message};
use chrono::{DateTime, Local, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Header {
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<FoundLink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub visualizations: Vec<FoundLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub datadeck: Managed,
    #[serde(flatten)]
    extra: serde_yaml_ng::Mapping,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Managed {
    pub id: i64,
    pub revision: i64,
    pub notes_revision: i64,
    metadata_hash: String,
    notes_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_kind: Option<Kind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered: Option<DiscoveredFiles>,
    #[serde(flatten)]
    extra: serde_yaml_ng::Mapping,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct DiscoveredFiles {
    count: usize,
    hash: String,
}

pub(super) struct Document {
    pub path: PathBuf,
    pub header: Header,
    pub notes: String,
    original: Option<[u8; 32]>,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..24].to_owned()
}

fn yaml_hash(value: impl Serialize) -> Result<String, String> {
    serde_yaml_ng::to_string(&value)
        .map(|yaml| hash(yaml.as_bytes()))
        .map_err(message)
}

impl Header {
    fn fingerprint(&self) -> Result<String, String> {
        yaml_hash((
            &self.url,
            &self.title,
            self.kind,
            &self.captured_at,
            &self.files,
            &self.images,
            &self.visualizations,
            &self.thumbnail,
            &self.extra,
        ))
    }

    pub fn time(&self) -> Option<i64> {
        let date = self.captured_at.as_deref()?;
        DateTime::parse_from_rfc3339(date)
            .map(|value| value.timestamp_millis())
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .ok()?
                    .and_hms_opt(0, 0, 0)
                    .map(|day| day.and_utc().timestamp_millis())
            })
    }

    pub fn set_time(&mut self, time: Option<i64>) {
        self.captured_at = time
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .map(|v| v.to_rfc3339_opts(SecondsFormat::Millis, true));
    }

    pub fn user_files(&self) -> &[String] {
        let count = self
            .datadeck
            .discovered
            .as_ref()
            .filter(|d| yaml_hash(&self.files).ok().as_ref() == Some(&d.hash))
            .map_or(0, |d| d.count);
        &self.files[..self.files.len().saturating_sub(count)]
    }

    pub fn set_files(
        &mut self,
        mut user: Vec<String>,
        discovered: Vec<String>,
    ) -> Result<(), String> {
        let user_count = user.len();
        for link in discovered {
            if !user.contains(&link) {
                user.push(link);
            }
        }
        let count = user.len() - user_count;
        self.datadeck.discovered = if count == 0 {
            None
        } else {
            Some(DiscoveredFiles {
                count,
                hash: yaml_hash(&user)?,
            })
        };
        self.files = user;
        Ok(())
    }

    pub fn automatic_title(&self) -> bool {
        self.title.is_empty()
            || self.datadeck.auto_title.as_deref() == Some(hash(self.title.as_bytes()).as_str())
    }

    pub fn set_page_title(&mut self, title: String) {
        if self.automatic_title() {
            self.datadeck.auto_title = (!title.is_empty()).then(|| hash(title.as_bytes()));
            self.title = title;
            self.datadeck.page_title = None;
        } else {
            self.datadeck.page_title = (!title.is_empty()).then_some(title);
        }
    }

    pub fn set_manual_title(&mut self, title: String) {
        let cached = if self.automatic_title() {
            Some(self.title.clone())
        } else {
            self.datadeck.page_title.take()
        };
        self.title = title;
        self.datadeck.auto_title = None;
        if let Some(page) = cached {
            self.set_page_title(page);
        }
    }

    pub fn automatic_kind(&self) -> bool {
        self.kind.is_none() || self.kind == self.datadeck.auto_kind
    }

    pub fn set_auto_kind(&mut self, kind: Kind) {
        self.kind = Some(kind);
        self.datadeck.auto_kind = Some(kind);
    }
}

fn split_markdown(raw: &str) -> Result<(Header, &str), String> {
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = text.split_inclusive('\n');
    let opening = lines.next().unwrap_or("");
    if opening.trim_end() != "---" {
        return Ok((Header::default(), text));
    }
    let mut offset = opening.len();
    for line in lines {
        if matches!(line.trim_end(), "---" | "...") {
            let yaml = &text[opening.len()..offset];
            let header = if yaml.trim().is_empty() {
                Header::default()
            } else {
                serde_yaml_ng::from_str(yaml).map_err(message)?
            };
            let body = &text[offset + line.len()..];
            return Ok((
                header,
                body.strip_prefix("\r\n")
                    .or_else(|| body.strip_prefix('\n'))
                    .unwrap_or(body),
            ));
        }
        offset += line.len();
    }
    Err("YAML frontmatter has no closing ---.".into())
}

fn web_links(values: &[FoundLink]) -> Vec<FoundLink> {
    values
        .iter()
        .filter_map(|item| {
            let url = http_url(&item.url).ok()?;
            Some(FoundLink {
                title: if item.title.trim().is_empty() {
                    url.as_str().to_owned()
                } else {
                    item.title.clone()
                },
                url: url.into(),
            })
        })
        .collect()
}

fn native_id(url: &str) -> i64 {
    let digest = Sha256::digest(url.as_bytes());
    let n = i64::from_be_bytes(digest[..8].try_into().expect("eight bytes"));
    n | i64::MIN
}

fn modified_version(modified: SystemTime) -> i64 {
    modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(i64::MAX as u128) as i64
}

pub(crate) fn markdown(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| !s.starts_with('.'))
        && path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
}

impl Document {
    pub fn read(path: PathBuf) -> Result<Option<Self>, String> {
        let metadata = fs::symlink_metadata(&path).map_err(message)?;
        if !metadata.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path).map_err(message)?;
        let (mut header, body) = split_markdown(&raw)?;
        if header.url.trim().is_empty() {
            let Some(found) = crate::capture::capture_text(body) else {
                return Ok(None);
            };
            header.url = found.url;
        }
        header.url = http_url(&header.url)?.into();
        if header.captured_at.is_some() && header.time().is_none() {
            return Err("Invalid captured_at date.".into());
        }
        let version = modified_version(metadata.modified().unwrap_or(UNIX_EPOCH));
        if header.datadeck.id == 0 {
            header.datadeck.id = native_id(&header.url);
        }
        if header.datadeck.metadata_hash != header.fingerprint()? {
            header.datadeck.revision = version.max(header.datadeck.revision.saturating_add(1));
        }
        if header.datadeck.notes_hash != hash(body.as_bytes()) {
            header.datadeck.notes_revision =
                version.max(header.datadeck.notes_revision.saturating_add(1));
        }
        Ok(Some(Self {
            path,
            header,
            notes: body.to_owned(),
            original: Some(Sha256::digest(raw.as_bytes()).into()),
        }))
    }

    pub fn new(folder: &Path, header: Header, notes: String) -> Self {
        let url = http_url(&header.url).expect("normalized bookmark");
        let basis = if !header.title.is_empty() {
            &header.title
        } else {
            url.path_segments()
                .and_then(|mut s| s.rfind(|p| !p.is_empty()))
                .or_else(|| url.host_str())
                .unwrap_or("bookmark")
        };
        let slug = basis
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(8)
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
        let slug = slug.chars().take(45).collect::<String>();
        let date = header
            .time()
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .map(|d| d.format("%Y-%m-%d-").to_string())
            .unwrap_or_default();
        let name = format!(
            "{date}{}-{}.md",
            if slug.is_empty() { "bookmark" } else { &slug },
            header.datadeck.id
        );
        let mut path = folder.join(&name);
        let stem = name.trim_end_matches(".md");
        let mut suffix = 2;
        while fs::symlink_metadata(&path).is_ok() {
            path = folder.join(format!("{stem}-{suffix}.md"));
            suffix += 1;
        }
        Self {
            path,
            header,
            notes,
            original: None,
        }
    }

    pub fn unchanged(&self) -> Result<(), String> {
        if let Some(original) = self.original {
            let metadata = fs::symlink_metadata(&self.path).map_err(message)?;
            if !metadata.is_file()
                || <[u8; 32]>::from(Sha256::digest(fs::read(&self.path).map_err(message)?))
                    != original
            {
                return Err(format!(
                    "{} changed in another editor. Reload and try again.",
                    self.path.display()
                ));
            }
        }
        Ok(())
    }

    pub fn write(&mut self) -> Result<(), String> {
        self.header.datadeck.metadata_hash = self.header.fingerprint()?;
        self.header.datadeck.notes_hash = hash(self.notes.as_bytes());
        let yaml = serde_yaml_ng::to_string(&self.header).map_err(message)?;
        let content = format!("---\n{yaml}---\n\n{}", self.notes);
        let directory = self.path.parent().ok_or("Missing inbox directory.")?;
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(message)?;
        if self.original.is_some() {
            file.as_file()
                .set_permissions(
                    fs::symlink_metadata(&self.path)
                        .map_err(message)?
                        .permissions(),
                )
                .map_err(message)?;
        }
        file.write_all(content.as_bytes()).map_err(message)?;
        file.as_file().sync_all().map_err(message)?;
        self.unchanged()?;
        if self.original.is_some() {
            file.persist(&self.path)
        } else {
            file.persist_noclobber(&self.path)
        }
        .map_err(message)?;
        File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(message)?;
        self.original = Some(Sha256::digest(content.as_bytes()).into());
        Ok(())
    }

    pub fn bookmark(&self) -> Bookmark {
        let h = &self.header;
        let thumbnail = h.thumbnail.as_ref().and_then(|value| {
            let mut parts = Path::new(value).components();
            if !matches!(parts.next(), Some(Component::Normal(s)) if s=="assets")
                || !matches!(parts.next(), Some(Component::Normal(_)))
                || parts.next().is_some()
            {
                return None;
            }
            let folder = self.path.parent()?;
            if !fs::symlink_metadata(folder.join("assets")).ok()?.is_dir() {
                return None;
            }
            let path = folder.join(value);
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
                return None;
            }
            fs::read(path).ok()
        });
        let captured_at = h.time();
        Bookmark {
            id: h.datadeck.id,
            revision: h.datadeck.revision,
            notes_revision: h.datadeck.notes_revision,
            path: self.path.clone(),
            captured_at,
            capture_label: captured_at
                .and_then(DateTime::<Utc>::from_timestamp_millis)
                .map(|v| {
                    v.with_timezone(&Local)
                        .format("%Y-%m-%d · %H:%M")
                        .to_string()
                })
                .unwrap_or_default(),
            url: h.url.clone(),
            title: h.title.clone(),
            kind: h.kind.unwrap_or_default(),
            notes: self.notes.clone(),
            files: h
                .files
                .iter()
                .filter(|s| Path::new(s).is_absolute() || http_url(s).is_ok())
                .cloned()
                .collect(),
            images: web_links(&h.images),
            visualizations: web_links(&h.visualizations),
            image: thumbnail,
        }
    }
}

pub(super) fn asset(folder: &Path, id: i64, image: &[u8]) -> Result<String, String> {
    let directory = folder.join("assets");
    fs::create_dir_all(&directory).map_err(message)?;
    if !fs::symlink_metadata(&directory).map_err(message)?.is_dir() {
        return Err("assets must be a directory inside the inbox.".into());
    }
    let name = format!("{id}-{}.png", hash(image));
    let target = directory.join(&name);
    if !target.exists() {
        let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(message)?;
        file.write_all(image).map_err(message)?;
        file.as_file().sync_all().map_err(message)?;
        file.persist_noclobber(target).map_err(message)?;
        File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(message)?;
    }
    Ok(format!("assets/{name}"))
}

pub(super) fn scan(folder: &Path) -> Result<(Vec<Document>, Vec<String>), String> {
    let mut paths = vec![];
    for entry in fs::read_dir(folder).map_err(message)? {
        let path = entry.map_err(message)?.path();
        if markdown(&path) {
            paths.push(path);
        }
    }
    paths.sort();
    let mut docs = vec![];
    let mut warnings = vec![];
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    for path in paths {
        match Document::read(path.clone()) {
            Ok(Some(doc))
                if ids.insert(doc.header.datadeck.id) && urls.insert(doc.header.url.clone()) =>
            {
                docs.push(doc)
            }
            Ok(Some(_)) => warnings.push(format!(
                "{} repeats another bookmark's id or URL.",
                path.display()
            )),
            Ok(None) => {}
            Err(error) => warnings.push(format!("{}: {error}", path.display())),
        }
    }
    Ok((docs, warnings))
}
