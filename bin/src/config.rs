use anyhow::Context;
use lib::plugin::jsonfeed::FeedConfig;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::glob::GlobCache;
use crate::rhai_plugin::compile_rhai_dir;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IgnoreMode {
    Extend,
    Replace,
    Remove,
}

/// Config for building/serving the rendered files.
#[derive(Clone, Debug, Deserialize)]
pub struct BuildConfig {
    pub ignore: Option<Vec<String>>,
    pub ignore_mode: Option<IgnoreMode>,
    pub minify: Option<bool>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            ignore: Some(vec!["*.toml".to_string()]),
            ignore_mode: Some(IgnoreMode::Extend),
            minify: Some(false),
        }
    }
}

#[derive(Deserialize)]
pub struct TomlConfig {
    pub templates: Option<String>,
    pub template: Option<String>,
    pub highlighting_themes: Option<Vec<String>>,
    pub build: Option<BuildConfig>,
    pub plugins_dir: Option<String>,
    pub filters_dir: Option<String>,
    pub functions_dir: Option<String>,
    pub feed: Option<FeedConfig>,
}

#[derive(Clone, Debug)]
pub struct RhaiScript {
    pub name: String,
    pub ast: rhai::AST,
}

/// Config for the actual templates
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub templates: Option<PathBuf>,
    pub template: Option<String>,
    pub highlighting_themes: Option<Vec<String>>,
    pub build: BuildConfig,
    pub plugins: Vec<RhaiScript>,
    pub filters: Vec<RhaiScript>,
    pub functions: Vec<RhaiScript>,
    pub feed: Option<FeedConfig>,
}

impl Config {
    pub fn new(toml: PathBuf) -> anyhow::Result<Self> {
        let s = fs::read_to_string(&toml).with_context(|| format!("reading {}", toml.display()))?;
        let cfg: TomlConfig =
            toml::from_str(&s).with_context(|| format!("parsing {}", toml.display()))?;

        let base_dir = toml.parent().unwrap_or(Path::new("."));

        let templates_resolved = cfg.templates.map(|t| {
            let p = PathBuf::from(t);
            if p.is_absolute() {
                p
            } else {
                let parent = base_dir.to_path_buf();
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
            highlighting_themes: cfg.highlighting_themes,
            build: cfg.build.unwrap_or_default(),
            plugins: compile_rhai_dir(cfg.plugins_dir, base_dir),
            filters: compile_rhai_dir(cfg.filters_dir, base_dir),
            functions: compile_rhai_dir(cfg.functions_dir, base_dir),
            feed: cfg.feed,
        })
    }

    pub fn merged(parent: Option<&Config>, local: Option<Config>) -> Config {
        match (parent, local) {
            (None, None) => Config::default(),
            (Some(p), None) => p.clone(),
            (None, Some(c)) => c,
            (Some(parent), Some(mut local)) => {
                local.build = merge_build_config(&parent.build, &local.build);

                local.templates = local.templates.or_else(|| parent.templates.clone());
                local.template = local.template.or_else(|| parent.template.clone());
                local.highlighting_themes = local
                    .highlighting_themes
                    .or_else(|| parent.highlighting_themes.clone());

                local
            }
        }
    }

    pub fn is_ignored(&self, path: &Path, glob_cache: &mut GlobCache) -> bool {
        match &self.build.ignore {
            Some(patterns) => glob_cache.is_match(patterns, path).unwrap_or(false),
            None => false,
        }
    }
}

fn merge_build_config(parent: &BuildConfig, local: &BuildConfig) -> BuildConfig {
    let mode = local
        .ignore_mode
        .clone()
        .or_else(|| parent.ignore_mode.clone())
        .unwrap_or(IgnoreMode::Extend);

    let parent_ignores = parent.ignore.clone().unwrap_or_default();
    let local_ignores = local.ignore.clone().unwrap_or_default();

    let merged_ignore = match mode {
        IgnoreMode::Extend => {
            let mut v = parent_ignores;
            v.extend(local_ignores);
            v
        }
        IgnoreMode::Replace => local_ignores,
        IgnoreMode::Remove => parent_ignores
            .into_iter()
            .filter(|p| !local_ignores.contains(p))
            .collect(),
    };

    BuildConfig {
        ignore: if merged_ignore.is_empty() {
            None
        } else {
            Some(merged_ignore)
        },
        ignore_mode: Some(mode),
        minify: local.minify.or(parent.minify),
    }
}

#[hotpath::measure]
pub fn load_overrides(
    md_path: &Path,
    watch_dir: &Path,
    parent: Option<&Config>,
) -> anyhow::Result<Config> {
    let local = find_local_config(md_path, watch_dir)
        .map(Config::new)
        .transpose()?;

    Ok(Config::merged(parent, local))
}

fn find_local_config(md_path: &Path, watch_dir: &Path) -> Option<PathBuf> {
    let dir = if md_path.is_dir() {
        md_path.canonicalize().ok()?
    } else {
        md_path.parent().unwrap_or(watch_dir).to_path_buf()
    };

    // Page-specific override (foo.md → foo.toml)
    if let Some(stem) = md_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| *s != "index.md")
    {
        let mut page = dir.join(stem);
        page.set_extension(".toml");
        if page.is_file() {
            return Some(page);
        }
    }

    // Directory override (index.toml)
    let index = dir.join("index.toml");
    if index.is_file() {
        return Some(index);
    }

    None
}
