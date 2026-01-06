use anyhow::Context;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct TomlConfig {
    pub templates: Option<String>,
    pub template: Option<String>,
    pub highlighting_themes: Option<Vec<String>>
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub templates: Option<PathBuf>,
    pub template: Option<String>,
    pub highlighting_themes: Option<Vec<String>>
}

impl Config {
    pub fn new(toml: PathBuf) -> anyhow::Result<Self> {
        let s = fs::read_to_string(&toml).with_context(|| format!("reading {}", toml.display()))?;
        let cfg: TomlConfig =
            toml::from_str(&s).with_context(|| format!("parsing {}", toml.display()))?;

        let templates_resolved = cfg.templates.map(|t| {
            let p = PathBuf::from(t);
            if p.is_absolute() {
                p
            } else {
                let parent = toml
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                parent
                    .join(&p)
                    .canonicalize()
                    .with_context(|| format!("reading {}", p.display()))
                    .unwrap()
            }
        });

        Ok(Self {
            templates: templates_resolved,
            template: cfg.template,
            highlighting_themes: cfg.highlighting_themes
        })
    }
}

pub fn load_overrides(
    md_path: &Path,
    watch_dir: &Path,
) -> anyhow::Result<Config> {
    let dir = if md_path.is_dir() {
        md_path.to_path_buf()
    } else {
        md_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| watch_dir.to_path_buf())
    };

    let file_name = md_path.file_name().and_then(|s| s.to_str());
    let stem_opt = match file_name {
        Some("index.md") => None,
        Some(_) => md_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string()),
        None => None,
    };

    if let Some(stem) = &stem_opt {
        let page_toml = dir.join(format!("{}.toml", stem));
        if page_toml.is_file() {
            return Config::new(page_toml);
        }
    }

    let index_toml = dir.join("index.toml");
    if index_toml.is_file() {
        return Config::new(index_toml);
    }

    Ok(Config::default())
}
