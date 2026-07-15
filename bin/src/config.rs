use anyhow::{Context, Result};
use lib::plugin::jsonfeed::FeedConfig;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::glob::GlobCache;
use crate::rhai_plugin::compile_rhai_dir;
use crate::utils::resolve_path;

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IgnoreMode {
    #[default]
    Extend,
    Replace,
    Remove,
}

/// Config for building/serving the rendered files.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    pub output_dir: Option<PathBuf>,
    pub ignore: Option<Vec<String>>,
    pub ignore_mode: Option<IgnoreMode>,
    pub minify: Option<bool>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            output_dir: Some("out".to_owned().into()),
            ignore: Some(vec!["*.toml".to_string()]),
            ignore_mode: Some(IgnoreMode::default()),
            minify: Some(false),
        }
    }
}

#[derive(Deserialize)]
pub struct TomlConfig {
    pub layouts: Option<String>,
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

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub layouts: Option<PathBuf>,
    pub template: Option<String>,
    pub highlighting_themes: Option<Vec<String>>,
    pub build: BuildConfig,
    pub plugins: Vec<RhaiScript>,
    pub filters: Vec<RhaiScript>,
    pub functions: Vec<RhaiScript>,
    pub feed: Option<FeedConfig>,
}

#[hotpath::measure_all]
impl Config {
    pub fn new(toml: PathBuf) -> Result<Self> {
        let s = fs::read_to_string(&toml).with_context(|| format!("reading {}", toml.display()))?;
        let cfg: TomlConfig =
            toml::from_str(&s).with_context(|| format!("parsing {}", toml.display()))?;

        let base_dir = toml.parent().unwrap_or_else(|| Path::new("."));

        let layouts = resolve_path(cfg.layouts, base_dir);
        let mut build = cfg.build.unwrap_or_default();
        build.output_dir = resolve_path(build.output_dir, base_dir);

        Ok(Self {
            layouts,
            template: cfg.template,
            highlighting_themes: cfg.highlighting_themes,
            build,
            plugins: compile_rhai_dir(cfg.plugins_dir, base_dir),
            filters: compile_rhai_dir(cfg.filters_dir, base_dir),
            functions: compile_rhai_dir(cfg.functions_dir, base_dir),
            feed: cfg.feed,
        })
    }

    pub fn merged(parent: Option<Config>, local: Option<Config>, cli: Option<Config>) -> Config {
        let cli_local = match (cli, local) {
            (None, None) => Config::default(),
            (Some(c), None) => c,
            (None, Some(l)) => l,
            (Some(cli), Some(mut local)) => {
                local.build = merge_build_config(&cli.build, &local.build);
                local.layouts = cli.layouts.clone().or(local.layouts);
                local.template = cli.template.clone().or(local.template);
                local.highlighting_themes = cli
                    .highlighting_themes
                    .clone()
                    .or(local.highlighting_themes);
                local.feed = cli.feed.clone().or(local.feed);
                local
            }
        };

        match (parent, Some(cli_local)) {
            (None, Some(cl)) => cl,
            (Some(p), Some(mut cl)) => {
                cl.build = merge_build_config(&p.build, &cl.build);
                cl.layouts = cl.layouts.or_else(|| p.layouts.clone());
                cl.template = cl.template.or_else(|| p.template.clone());
                cl.highlighting_themes = cl
                    .highlighting_themes
                    .or_else(|| p.highlighting_themes.clone());
                cl.feed = cl.feed.or_else(|| p.feed.clone());
                cl
            }
            _ => unreachable!(),
        }
    }
}

#[hotpath::measure]
fn merge_build_config(parent: &BuildConfig, local: &BuildConfig) -> BuildConfig {
    let mode = local
        .ignore_mode
        .clone()
        .or_else(|| parent.ignore_mode.clone())
        .unwrap_or_default();

    let parent_ignores = parent.ignore.as_ref().cloned().unwrap_or_default();
    let local_ignores = local.ignore.as_ref().cloned().unwrap_or_default();

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
        output_dir: local
            .output_dir
            .as_ref()
            .or(parent.output_dir.as_ref())
            .cloned(),
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
    parent: Option<Config>,
    cli: Option<Config>,
) -> Result<RuntimeConfig> {
    let local = find_local_config(md_path, watch_dir)
        .map(Config::new)
        .transpose()?;

    let merged = Config::merged(parent, local, cli);

    let runtime_config = RuntimeConfig::try_from(merged)?;

    Ok(runtime_config)
}

#[hotpath::measure]
fn find_local_config(md_path: &Path, watch_dir: &Path) -> Option<PathBuf> {
    let dir = if md_path.is_dir() {
        md_path.canonicalize().ok()?
    } else {
        md_path.parent().unwrap_or(watch_dir).to_path_buf()
    };

    // Page-specific override (foo.md -> foo.toml)
    if let Some(stem) = md_path.file_stem().and_then(|s| s.to_str())
        && stem != "index"
    {
        let page = dir.join(stem).with_extension("toml");
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

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub layouts: PathBuf,
    pub template: String,
    pub highlighting_themes: Option<Vec<String>>,
    pub build: BuildConfig,
    pub plugins: Vec<RhaiScript>,
    pub filters: Vec<RhaiScript>,
    pub functions: Vec<RhaiScript>,
    pub feed: Option<FeedConfig>,
}

impl RuntimeConfig {
    pub fn is_ignored(&self, path: &Path, glob_cache: &mut GlobCache) -> bool {
        let absolute_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        self.build.ignore.as_ref().is_some_and(|patterns| {
            glob_cache
                .is_match(patterns, &absolute_path)
                .unwrap_or(false)
        })
    }
}

impl TryFrom<Config> for RuntimeConfig {
    type Error = anyhow::Error;

    fn try_from(cfg: Config) -> Result<Self, Self::Error> {
        let layouts = cfg.layouts.ok_or_else(|| {
            anyhow::anyhow!("Configuration Error: `layouts` directory is not defined.")
        })?;

        let template = cfg.template.ok_or_else(|| {
            anyhow::anyhow!("Configuration Error: default `template` is not defined.")
        })?;

        let mut build = cfg.build;
        let mut ignore = build.ignore.unwrap();
        ignore.push(format!("{}/**/*.tera", layouts.to_str().unwrap()));
        ignore.push(format!(
            "{}/**",
            build
                .output_dir
                .as_ref()
                .and_then(|dir| dir.to_str())
                .unwrap()
        ));
        build.ignore = Some(ignore);

        Ok(Self {
            layouts,
            template,
            highlighting_themes: cfg.highlighting_themes,
            build,
            plugins: cfg.plugins,
            filters: cfg.filters,
            functions: cfg.functions,
            feed: cfg.feed,
        })
    }
}
