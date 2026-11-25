use super::{FoundLink, Kind, dataset_url, file_url, preview::linked};
use scraper::{ElementRef, Html, Selector};
use url::Url;

const MAX_LINKS: usize = 32;

fn image_url(url: &Url) -> bool {
    let Some((_, suffix)) = url.path().rsplit_once('.') else {
        return false;
    };
    matches!(
        suffix.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "svg" | "avif"
    )
}

fn chart_url(url: &Url) -> bool {
    let domain = url.host_str().unwrap_or_default();
    let host =
        |name: &str| domain == name || domain.strip_suffix(name).is_some_and(|s| s.ends_with('.'));
    let path = url.path();
    (host("ourworldindata.org")
        && (path.starts_with("/grapher/") || path.starts_with("/explorers/")))
        || (host("dwcdn.net") && path.len() > 1)
        || (host("datawrapper.de") && (path.starts_with("/_/") || path.starts_with("/chart/")))
        || ((host("flourish.studio") || host("flo.uri.sh"))
            && (path.contains("/visualisation/") || path.contains("/story/")))
        || (host("tableau.com")
            && (path.contains("/views/") || url.fragment().is_some_and(|s| s.contains("/views/"))))
        || (host("observablehq.com")
            && (path.starts_with("/embed/") || path.starts_with("/@") || path.starts_with("/d/")))
}

fn compact(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn record(links: &mut Vec<FoundLink>, url: Url, title: &str) {
    if url.as_str().len() > 4096 {
        return;
    }
    let fallback = url
        .path_segments()
        .and_then(|mut parts| parts.rfind(|s| !s.is_empty()))
        .unwrap_or_else(|| url.host_str().unwrap_or("Link"));
    if let Some(existing) = links.iter_mut().find(|item| item.url == url.as_str()) {
        if existing.title == fallback && !title.is_empty() {
            existing.title = compact(title);
        }
    } else if links.len() < MAX_LINKS {
        links.push(FoundLink {
            title: compact(if title.is_empty() { fallback } else { title }),
            url: url.into(),
        });
    }
}

fn element_title(element: ElementRef<'_>, caption: &Selector) -> String {
    for attr in ["aria-label", "alt", "title"] {
        if let Some(label) = element.attr(attr).map(str::trim).filter(|s| !s.is_empty()) {
            return compact(label);
        }
    }
    if element.value().name() == "a" {
        let text = compact(&element.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty() {
            return text;
        }
    }
    std::iter::once(element)
        .chain(element.ancestors().filter_map(ElementRef::wrap))
        .find(|e| e.value().name() == "figure")
        .and_then(|figure| figure.select(caption).next())
        .map(|caption| compact(&caption.text().collect::<Vec<_>>().join(" ")))
        .unwrap_or_default()
}

pub(super) fn assets(document: &Html, base: &Url) -> (Vec<FoundLink>, Vec<FoundLink>) {
    let selector = Selector::parse("meta[content], img[src], img[data-src], a[href], iframe[src], tableau-viz[src], [data-grapher-src], [data-explorer-src], .flourish-embed[data-src]").unwrap();
    let caption = Selector::parse("figcaption").unwrap();
    let mut images = Vec::new();
    let mut charts = Vec::new();
    for element in document.select(&selector) {
        let tag = element.value().name();
        let owid = element
            .attr("data-grapher-src")
            .or_else(|| element.attr("data-explorer-src"));
        let url = match tag {
            "meta" => {
                let property = element
                    .attr("property")
                    .or_else(|| element.attr("name"))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(
                    property.as_str(),
                    "og:image"
                        | "og:image:url"
                        | "og:image:secure_url"
                        | "twitter:image"
                        | "twitter:image:src"
                ) {
                    continue;
                }
                element.attr("content").and_then(|src| linked(base, src))
            }
            "a" => element.attr("href").and_then(|src| linked(base, src)),
            "img" => ["data-src", "src"]
                .into_iter()
                .find_map(|attr| element.attr(attr).and_then(|s| linked(base, s))),
            _ if owid.is_some() => owid.and_then(|src| linked(base, src)),
            _ if element.value().classes().any(|c| c == "flourish-embed") => {
                // Flourish's script embed stores a relative identifier on its container.
                element
                    .attr("data-src")
                    .and_then(|s| s.trim_matches('/').split_once('/'))
                    .and_then(|(kind, id)| {
                        (matches!(kind, "visualisation" | "story")
                            && !id.is_empty()
                            && id.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| {
                            Url::parse(&format!("https://public.flourish.studio/{kind}/{id}/")).ok()
                        })
                        .flatten()
                    })
            }
            _ => element.attr("src").and_then(|src| linked(base, src)),
        };
        let Some(mut url) = url else { continue };
        if tag == "img"
            && ["width", "height"].iter().any(|key| {
                element
                    .attr(key)
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|v| v <= 2)
            })
        {
            continue;
        }
        let label = element_title(element, &caption);
        let label_words = label.to_ascii_lowercase();
        let labeled_chart = label_words.split(|c: char| !c.is_alphabetic()).any(|word| {
            matches!(
                word,
                "chart" | "graph" | "visualization" | "visualisation" | "dashboard"
            )
        });
        if chart_url(&url)
            || owid.is_some()
            || tag == "tableau-viz"
            || (matches!(tag, "iframe" | "a") && labeled_chart)
        {
            record(&mut charts, url.clone(), &label);
        }
        if matches!(tag, "meta" | "img")
            || (tag == "a"
                && (image_url(&url)
                    || element
                        .attr("type")
                        .is_some_and(|t| t.starts_with("image/"))))
        {
            url.set_fragment(None);
            record(&mut images, url, &label);
        }
    }
    (images, charts)
}

pub(super) fn url_kind(url: &Url) -> Kind {
    if dataset_url(url) {
        Kind::Dataset
    } else if chart_url(url) {
        Kind::Visualization
    } else {
        Kind::Other
    }
}

fn schema_type(name: &str) -> Kind {
    let name = name
        .strip_prefix("https://schema.org/")
        .or_else(|| name.strip_prefix("http://schema.org/"))
        .unwrap_or(name);
    match name {
        "Dataset" => Kind::Dataset,
        "Article" | "NewsArticle" | "Report" | "BlogPosting" | "ScholarlyArticle" => Kind::Story,
        _ => Kind::Other,
    }
}

fn schema_nodes<'a>(value: &'a serde_json::Value, result: &mut Vec<&'a serde_json::Value>) {
    if let Some(list) = value.as_array() {
        for item in list {
            schema_nodes(item, result);
        }
    } else if value.is_object() {
        result.push(value);
        if let Some(graph) = value.get("@graph") {
            schema_nodes(graph, result);
        }
    }
}

pub(super) fn page_kind(document: &Html, page: &Url, base: &Url, files: &mut Vec<String>) -> Kind {
    let mut kind = url_kind(page);
    let selector =
        Selector::parse("meta[property], [itemtype], script[type='application/ld+json']").unwrap();
    for element in document.select(&selector) {
        let mut types = vec![];
        if element
            .attr("property")
            .is_some_and(|p| p.eq_ignore_ascii_case("og:type"))
            && element
                .attr("content")
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("article"))
        {
            types.push(Kind::Story);
        }
        if let Some(itemtype) = element.attr("itemtype") {
            types.extend(itemtype.split_whitespace().map(schema_type));
        }
        if element.value().name() == "script"
            && let Ok(json) = serde_json::from_str(&element.inner_html())
        {
            let mut nodes = Vec::new();
            schema_nodes(&json, &mut nodes);
            for node in nodes {
                let Some(data_type) = node.get("@type") else {
                    continue;
                };
                let node_types: Vec<_> = data_type
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(std::slice::from_ref(data_type))
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(schema_type)
                    .collect();
                if node_types.contains(&Kind::Dataset)
                    && let Some(distribution) = node.get("distribution")
                {
                    for file in distribution
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(std::slice::from_ref(distribution))
                    {
                        let Some(url) = file
                            .get("contentUrl")
                            .and_then(|u| u.as_str())
                            .and_then(|s| linked(base, s))
                        else {
                            continue;
                        };
                        let format = file
                            .get("encodingFormat")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        if files.len() < 64
                            && (dataset_url(&url)
                                || matches!(
                                    format.as_str(),
                                    "csv"
                                        | "tsv"
                                        | "parquet"
                                        | "xls"
                                        | "xlsx"
                                        | "text/csv"
                                        | "text/tab-separated-values"
                                        | "application/vnd.apache.parquet"
                                        | "application/vnd.ms-excel"
                                        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                                ))
                        {
                            let url = file_url(url);
                            if !files.contains(&url) {
                                files.push(url);
                            }
                        }
                    }
                }
                types.extend(node_types);
            }
        }
        for inferred in types {
            if kind == Kind::Other || (kind == Kind::Story && inferred == Kind::Dataset) {
                kind = inferred;
            }
        }
    }
    kind
}
