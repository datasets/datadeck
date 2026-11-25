use serde::Deserialize;
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};
use url::Url;

const DESCRIPTORS: [&str; 3] = ["datapackage.json", "datapackage.yaml", "datapackage.yml"];
const FORMATS: [&str; 5] = ["csv", "tsv", "xls", "xlsx", "parquet"];
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 50_000;
const MAX_RESOURCES: usize = 2_000;
const MAX_DESCRIPTOR_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub resources: Vec<Resource>,
}

pub fn is_manifest(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|file| {
            DESCRIPTORS
                .iter()
                .any(|name| file.eq_ignore_ascii_case(name))
        })
}

pub fn discover(path: &Path) -> Result<Workspace, String> {
    if is_manifest(path) && path.is_file() {
        // Resolve resource paths beside the selected descriptor, including a symlinked descriptor.
        let absolute = std::path::absolute(path).map_err(|err| err.to_string())?;
        return read_manifest(&absolute);
    }
    let path = path
        .canonicalize()
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if path.is_dir() {
        if let Some(manifest) = DESCRIPTORS
            .iter()
            .map(|file| path.join(file))
            .find(|file| file.is_file())
        {
            return read_manifest(&manifest);
        }
        return scan_folder(&path);
    }
    Err("Choose a folder or a datapackage.json, datapackage.yaml, or datapackage.yml file.".into())
}

#[derive(Deserialize)]
struct Descriptor {
    title: Option<String>,
    name: Option<String>,
    resources: Vec<DescriptorResource>,
}

#[derive(Deserialize)]
struct DescriptorResource {
    name: Option<String>,
    title: Option<String>,
    format: Option<String>,
    path: Option<ResourcePaths>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResourcePaths {
    One(String),
    Many(Vec<String>),
}

fn tabular(format: &str) -> bool {
    FORMATS
        .iter()
        .any(|known| format.eq_ignore_ascii_case(known))
}

fn tabular_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(tabular)
}

fn folder_name(root: &Path) -> String {
    root.file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn resource_source(
    root: &Path,
    value: &str,
    declared: Option<&str>,
) -> Result<Option<String>, String> {
    if let Ok(url) = Url::parse(value) {
        if !matches!(url.scheme(), "http" | "https") {
            return Ok(None);
        }
        return Ok(
            (tabular_path(Path::new(url.path())) || declared.is_some_and(tabular))
                .then(|| url.into()),
        );
    }
    if value.trim().is_empty() {
        return Ok(None);
    }
    let path = Path::new(value);
    if !tabular_path(path) && !declared.is_some_and(tabular) {
        return Ok(None);
    }
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "Resource path “{value}” must be relative to the package folder."
        ));
    }
    let absolute = root.join(path);
    Ok(Some(
        absolute
            .to_str()
            .ok_or("A resource has a non-UTF-8 path.")?
            .to_owned(),
    ))
}

fn read_manifest(path: &Path) -> Result<Workspace, String> {
    let root = path.parent().ok_or("Missing package folder.")?;
    let file = fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_DESCRIPTOR_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
        return Err("The Data Package descriptor exceeds 8 MiB.".into());
    }
    let is_json = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    let descriptor: Descriptor = if is_json {
        serde_json::from_slice(&bytes).map_err(|err| err.to_string())
    } else {
        serde_yaml_ng::from_slice(&bytes).map_err(|err| err.to_string())
    }
    .map_err(|err| format!("Could not read {}: {err}", path.display()))?;

    let mut resources = Vec::new();
    for resource in descriptor.resources {
        let paths = match resource.path {
            Some(ResourcePaths::One(path)) => vec![path],
            Some(ResourcePaths::Many(paths)) => paths,
            None => continue, // This viewer opens file-backed resources.
        };
        let label = resource
            .title
            .filter(|s| !s.trim().is_empty())
            .or(resource.name);
        let parts = paths.len();
        for (i, path) in paths.into_iter().enumerate() {
            let Some(source) = resource_source(root, &path, resource.format.as_deref())? else {
                continue;
            };
            let mut name = label.as_deref().unwrap_or(&path).to_owned();
            if parts > 1 {
                name.push_str(&format!(" · part {} of {}", i + 1, parts));
            }
            resources.push(Resource { name, source });
        }
    }
    if resources.is_empty() {
        return Err(
            "This package contains no file-backed CSV, TSV, Excel, or Parquet resources.".into(),
        );
    }
    let name = descriptor
        .title
        .filter(|s| !s.trim().is_empty())
        .or(descriptor.name)
        .unwrap_or_else(|| folder_name(root));
    Ok(Workspace { name, resources })
}

fn scan_folder(root: &Path) -> Result<Workspace, String> {
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut resources = Vec::new();
    let mut entries = 0;
    while let Some((dir, depth)) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))? {
            let entry = entry.map_err(|err| err.to_string())?;
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(format!(
                    "Folder scan reached {MAX_ENTRIES} entries. Open a smaller folder or a Data Package descriptor."
                ));
            }
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry.file_type().map_err(|err| err.to_string())?;
            let path = entry.path();
            if kind.is_dir() && depth < MAX_DEPTH && name != "target" && name != "node_modules" {
                pending.push((path, depth + 1));
            } else if kind.is_file() && tabular_path(&path) {
                let relative = path
                    .strip_prefix(root)
                    .expect("directory entry under scan root");
                resources.push(Resource {
                    name: relative.to_string_lossy().into_owned(),
                    source: path
                        .to_str()
                        .ok_or("A resource has a non-UTF-8 path.")?
                        .to_owned(),
                });
                if resources.len() > MAX_RESOURCES {
                    return Err(format!(
                        "More than {MAX_RESOURCES} tables found. Open a smaller folder or a Data Package descriptor."
                    ));
                }
            }
            // DirEntry::file_type never follows symlinks, including directory cycles.
        }
    }
    resources.sort_by(|a, b| a.name.cmp(&b.name));
    if resources.is_empty() {
        return Err(format!(
            "No CSV, TSV, Excel, or Parquet files found within {MAX_DEPTH} folder levels."
        ));
    }
    Ok(Workspace {
        name: folder_name(root),
        resources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_prefer_descriptors_keep_order_and_resolve_each_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::write(root.join("ignored.csv"), "x\n").unwrap();
        fs::write(
            root.join("datapackage.json"),
            r#"{
            "title":"Weather archive",
            "resources":[
                {"name":"forecast","path":"https://example.org/download?sheet=1","format":"xlsx"},
                {"title":"Stations","path":"data/stations.csv"},
                {"name":"readme","path":"README.md"},
                {"name":"notes","data":[["id"],[1]]}
            ]
        }"#,
        )
        .unwrap();
        let workspace = discover(&root).unwrap();
        assert_eq!(workspace.name, "Weather archive");
        assert_eq!(
            workspace.resources,
            vec![
                Resource {
                    name: "forecast".into(),
                    source: "https://example.org/download?sheet=1".into()
                },
                Resource {
                    name: "Stations".into(),
                    source: root.join("data/stations.csv").display().to_string()
                },
            ]
        );
        assert_eq!(workspace, discover(&root.join("datapackage.json")).unwrap());
    }

    #[test]
    fn yaml_packages_resolve_named_parts_and_report_broken_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        for extension in ["yaml", "yml"] {
            let manifest = temp.path().join(format!("datapackage.{extension}"));
            fs::write(&manifest,"name: surveys\nresources:\n  - name: sample\n    path: [north.tsv, south.tsv]\n  - name: remote\n    path: https://example.org/lake.parquet?year=2024\n").unwrap();
            let result = discover(&manifest).unwrap();
            assert_eq!(result.name, "surveys");
            assert_eq!(result.resources[1].name, "sample · part 2 of 2");
            assert!(result.resources[1].source.ends_with("south.tsv"));
            assert_eq!(result.resources.len(), 3);
            fs::write(&manifest, "resources: [").unwrap();
            assert!(discover(&manifest).unwrap_err().contains("Could not read"));
        }
    }

    #[test]
    fn folder_scan_skips_hidden_build_and_symlink_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for name in [
            "data",
            "other/deep",
            ".git",
            ".hidden",
            "target",
            "node_modules",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("table.tsv"), "a\tb").unwrap();
        }
        for name in [
            "file.CSV",
            "files.xls",
            "files.XLSX",
            "large.parquet",
            "readme.md",
            ".secret.csv",
        ] {
            fs::write(root.join(name), "").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root, root.join("data/cycle")).unwrap();
        let result = discover(root).unwrap();
        let names = result
            .resources
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "data/table.tsv",
                "file.CSV",
                "files.XLSX",
                "files.xls",
                "large.parquet",
                "other/deep/table.tsv"
            ]
        );
    }

    #[test]
    fn directory_depth_has_a_finite_bound() {
        let temp = tempfile::tempdir().unwrap();
        let deep = (0..=MAX_DEPTH).fold(temp.path().to_path_buf(), |path, _| path.join("deeper"));
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("buried.csv"), "x\n").unwrap();
        assert!(discover(temp.path()).unwrap_err().contains("folder levels"));
    }
}
