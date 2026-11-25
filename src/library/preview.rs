use super::{FoundLink, Kind, dataset_url, file_url, http_url, message};
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use image::{ImageFormat, ImageReader, Limits};
use scraper::{Html, Selector};
use std::io::{Cursor, Read};
use std::time::Duration;
use ureq::ResponseExt;
use url::Url;

const PAGE_BYTES: u64 = 768 * 1024;
const IMAGE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Metadata {
    pub title: String,
    pub kind: Kind,
    pub files: Vec<String>,
    pub images: Vec<FoundLink>,
    pub visualizations: Vec<FoundLink>,
}

fn decode_html(bytes: &[u8], http_charset: Option<&str>) -> String {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        return encoding.decode(&bytes[bom..]).0.into_owned();
    }
    let encoding = http_charset
        .and_then(|s| Encoding::for_label(s.as_bytes()))
        .or_else(|| {
            let start = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
            let doc = Html::parse_document(&start);
            let selector = Selector::parse("meta[charset]").unwrap();
            doc.select(&selector)
                .find_map(|m| Encoding::for_label(m.attr("charset")?.as_bytes()))
        })
        .unwrap_or_else(|| {
            if std::str::from_utf8(bytes).is_ok() {
                UTF_8
            } else {
                WINDOWS_1252
            }
        });
    encoding.decode(bytes).0.into_owned()
}

pub(super) fn linked(base: &Url, value: &str) -> Option<Url> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }
    http_url(base.join(value).ok()?.as_str()).ok()
}

fn parse(html: &str, page: &Url) -> (Metadata, Option<Url>) {
    let document = Html::parse_document(html);
    let select = |pattern| Selector::parse(pattern).expect("static HTML selector");
    let base = document
        .select(&select("base[href]"))
        .find_map(|e| linked(page, e.attr("href")?))
        .unwrap_or_else(|| page.clone());
    let mut title = None;
    let mut photo = None;
    for tag in document.select(&select("meta[content]")) {
        let key = tag
            .attr("property")
            .or_else(|| tag.attr("name"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let value = tag.attr("content").unwrap_or_default().trim();
        if value.is_empty() {
            continue;
        }
        if key == "og:title" && title.is_none() {
            title = Some(value.to_owned());
        }
        if photo.is_none()
            && matches!(
                key.as_str(),
                "og:image" | "og:image:secure_url" | "twitter:image" | "twitter:image:src"
            )
        {
            photo = linked(&base, value);
        }
    }
    let title = title
        .or_else(|| {
            document
                .select(&select("title"))
                .next()
                .map(|e| e.text().collect::<String>())
        })
        .unwrap_or_default();
    let title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect();
    let mut files = Vec::new();
    for element in document.select(&select("a[href], link[href]")) {
        let Some(url) = element.attr("href").and_then(|s| linked(&base, s)) else {
            continue;
        };
        let mime = element
            .attr("type")
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let downloadable = dataset_url(&url)
            || matches!(
                mime.as_str(),
                "text/csv"
                    | "text/tab-separated-values"
                    | "application/vnd.apache.parquet"
                    | "application/vnd.ms-excel"
                    | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            )
            || element
                .attr("download")
                .is_some_and(crate::library::dataset_name);
        if downloadable {
            let url = file_url(url);
            if !files.contains(&url) {
                files.push(url);
            }
            if files.len() >= 64 {
                break;
            }
        }
    }
    if photo.is_none() {
        photo = document
            .select(&select("article img[src], main img[src]"))
            .find_map(|e| linked(&base, e.attr("src")?));
    }
    let (images, visualizations) = super::discovery::assets(&document, &base);
    let kind = super::discovery::page_kind(&document, page, &base, &mut files);
    (
        Metadata {
            title,
            kind,
            files,
            images,
            visualizations,
        },
        photo,
    )
}

fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    let format = image::guess_format(bytes).ok()?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let mut png = Cursor::new(Vec::new());
    image
        .thumbnail(320, 200)
        .to_rgba8()
        .write_to(&mut png, ImageFormat::Png)
        .ok()?;
    Some(png.into_inner())
}

pub(super) fn fetch_image(agent: &ureq::Agent, url: &Url) -> Option<Vec<u8>> {
    let mut response = agent
        .get(url.as_str())
        .config()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .call()
        .ok()?;
    if !response.status().is_success()
        || response
            .body()
            .content_length()
            .is_some_and(|n| n > IMAGE_BYTES)
    {
        return None;
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(IMAGE_BYTES)
        .read_to_vec()
        .ok()?;
    thumbnail(&bytes)
}

pub(super) fn fetch(url: &Url) -> Result<(Metadata, Option<Url>), String> {
    if dataset_url(url) {
        return Ok((
            Metadata {
                kind: Kind::Dataset,
                files: vec![file_url(url.clone())],
                ..Default::default()
            },
            None,
        ));
    }
    let agent = super::net::agent(url);
    let mut response = agent
        .get(url.as_str())
        .config()
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(12)))
        .build()
        .call()
        .map_err(message)?;
    let page = http_url(&response.get_uri().to_string())?;
    let mime = response
        .body()
        .mime_type()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if dataset_url(&page)
        || matches!(
            mime.as_str(),
            "text/csv"
                | "text/tab-separated-values"
                | "application/vnd.apache.parquet"
                | "application/vnd.ms-excel"
                | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        )
    {
        return Ok((
            Metadata {
                kind: Kind::Dataset,
                files: vec![file_url(page)],
                ..Default::default()
            },
            None,
        ));
    }
    if !matches!(mime.as_str(), "text/html" | "application/xhtml+xml" | "") {
        return Ok((Metadata::default(), None));
    }
    let charset = response.body().charset().map(str::to_owned);
    let mut head = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(PAGE_BYTES)
        .read_to_end(&mut head)
        .map_err(message)?;
    Ok(parse(&decode_html(&head, charset.as_deref()), &page))
}
