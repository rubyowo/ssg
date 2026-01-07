use anyhow::Context;
use clap::{Parser, Subcommand};
use log::{error, info, warn};
use minify_html::{Cfg, minify};
use notify::Watcher;
use std::{path::PathBuf, sync::Arc};

use tokio::sync::Mutex;

use tower_livereload::LiveReloadLayer;

mod config;
mod glob;

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};

use lib::{
    PageContext, TemplateEngine, parse_md,
    plugin::{
        NodeKind, Plugin,
        math::math_plugin,
        pipeline::PluginPipeline,
        reading_time::{ReadingTimeContext, reading_time_plugin},
        syntax_highlighting::{HighlighterThemeContext, highlight_plugin},
        toc::{TocContext, toc_plugin},
    },
    render, render_template, run_pipelines, to_ast,
};

use crate::{config::Config, glob::GlobCache};

#[derive(Parser)]
#[command(about = "ssg")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        input_dir: PathBuf,

        #[arg(short, long, default_value = "out")]
        output_dir: PathBuf,

        #[arg(short = 'T', long, default_value = "templates")]
        templates: PathBuf,

        #[arg(short, long, default_value = "index.tera")]
        template: String,
    },

    Serve {
        #[arg(default_value = ".")]
        watch_dir: PathBuf,

        #[arg(short = 'T', long, default_value = "templates")]
        templates: PathBuf,

        #[arg(short, long, default_value = "index.tera")]
        template: String,

        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        #[arg(long, default_value = "3000")]
        port: u16,
    },
}

#[derive(Clone)]
pub struct WebState {
    pub templates: PathBuf,
    pub template: String,
    pub watch_dir: PathBuf,
    pub glob_cache: Arc<Mutex<GlobCache>>,
}

async fn render_markdown(
    content: String,
    templates: &PathBuf,
    template_name: &str,
    config: Config,
) -> anyhow::Result<String> {
    let (frontmatter, md_content) =
        parse_md(&content).with_context(|| format!("Failed to parse frontmatter: {}", content))?;
    let mut ast = to_ast(md_content.to_string());

    let mut reading_pipeline = PluginPipeline::<ReadingTimeContext>::new();
    reading_pipeline.register(Plugin {
        kind: NodeKind::Text,
        func: reading_time_plugin,
    });

    let mut toc_pipeline = PluginPipeline::<TocContext>::new();
    toc_pipeline.register(Plugin {
        kind: NodeKind::Heading,
        func: toc_plugin,
    });

    let mut highlighting_pipeline = PluginPipeline::<()>::new();
    highlighting_pipeline.register(Plugin {
        kind: NodeKind::Code,
        func: highlight_plugin,
    });

    let mut math_pipeline = PluginPipeline::<()>::new();
    math_pipeline.register(Plugin {
        kind: NodeKind::Code,
        func: math_plugin,
    });
    math_pipeline.register(Plugin {
        kind: NodeKind::InlineMath,
        func: math_plugin,
    });

    run_pipelines!(
        &mut ast,
        reading_pipeline,
        toc_pipeline,
        highlighting_pipeline,
        math_pipeline
    );

    let content_html = render(&ast);

    let page_ctx = PageContext {
        frontmatter,
        content: content_html.clone(),
        ast: ast.clone(),
    };

    let themes: Vec<String> = config.highlighting_themes.unwrap_or_else(|| {
        vec![
            "catppuccin_mocha".to_string(),
            "catppuccin_latte".to_string(),
        ]
    });

    let themes_css: String = themes
        .iter()
        .filter_map(|theme| match arborium_theme::builtin::THEMES.get(theme) {
            Some(theme_fn) => Some(theme_fn().to_css("pre")),
            None => {
                warn!("Unknown highlighting theme: {theme}");
                None
            }
        })
        .collect();

    let base_css = arborium_theme::theme::generate_base_css();

    let css = base_css + &themes_css;

    let hl_context = HighlighterThemeContext { themes, css };

    let template_engine = TemplateEngine::new(templates)?;

    let rendered = render_template!(template_engine, template_name, None, page => page_ctx, reading => reading_pipeline.context, toc => toc_pipeline.context, highlighter => hl_context).with_context(|| format!("Failed to render tera template: {}", content))?;

    Ok(rendered)
}

// #[axum::debug_handler]
async fn render_path_handler(
    State(state): State<WebState>,
    Path(path): Path<String>,
) -> Result<Response, AppError> {
    let mut requested = path.trim_matches('/').to_string();
    if requested.ends_with(".html") {
        requested = requested.trim_end_matches(".html").to_string();
        requested = format!("{}.md", requested);
    }

    let base = state.watch_dir.join(&requested);
    let md_path: PathBuf = if base.is_dir() {
        base.join("index.md")
    } else {
        base.clone()
    };

    let mut glob_cache = state.glob_cache.lock().await;

    let config = config::load_overrides(&md_path, &state.watch_dir, None)
        .context("Failed to read config: ")
        .map_err(AppError)?;

    let config_cloned = config.clone();

    if let Some(ignore) = config_cloned.build.ignore {
        if glob_cache.is_match(&ignore, &md_path).unwrap_or(false) {
            return Err(AppError::from(anyhow::anyhow!(
                "Ignored path: {}",
                md_path.display()
            )));
        }
    }

    let content = if md_path.is_file() {
        std::fs::read_to_string(&md_path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("file not found: {}", md_path.display()),
        ))
    }
    .with_context(|| format!("Failed to read file: {}", md_path.display()))?;

    if md_path.extension().and_then(|s| s.to_str()) == Some("md") {
        let final_templates = config_cloned.templates.unwrap_or(state.templates);
        let final_template = config_cloned.template.unwrap_or(state.template);

        let rendered = render_markdown(content, &final_templates, &final_template, config)
            .await
            .with_context(|| format!("Failed to render HTML: {}", md_path.display()))?;

        Ok(Html(rendered).into_response())
    } else {
        info!("{}", md_path.display());
        let body = Body::from(content);
        let mut headers = HeaderMap::new();
        let mime = mime_guess::from_path(md_path).first_or_octet_stream();
        let mime = mime.essence_str();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime)
                .with_context(|| format!("Mime type {mime} is not a valid HeaderValue"))?,
        );
        Ok((headers, body).into_response())
    }
}

fn collect_files(
    root: &std::path::Path,
    config: &Config,
    glob_cache: &mut GlobCache,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    let ignore_patterns = &config.build.ignore;

    let mut is_ignored = |path: &std::path::Path| -> bool {
        ignore_patterns
            .clone()
            .map(|pats| glob_cache.is_match(&pats, path).unwrap_or(false))
            .unwrap_or(false)
    };

    while let Some(dir) = stack.pop() {
        if is_ignored(&dir) {
            continue;
        }

        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                error!("failed to read dir {}: {}", dir.display(), e);
                continue;
            }
        };

        for entry in rd {
            match entry {
                Ok(ent) => {
                    let path = ent.path();
                    if is_ignored(&path) {
                        continue;
                    }
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        files.push(path);
                    }
                }
                Err(e) => {
                    error!("failed to read entry in {}: {}", dir.display(), e);
                }
            }
        }
    }
    Ok(files)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt::init();

    let mut glob_cache = GlobCache::default();

    match args.command {
        Commands::Build {
            input_dir,
            output_dir,
            templates,
            template,
        } => {
            if !input_dir.is_dir() {
                error!("input path is not a directory: {}", input_dir.display());
                return Err(anyhow::anyhow!("input path is not a directory"));
            }

            std::fs::create_dir_all(&output_dir)?;

            let root_config = config::load_overrides(
                &input_dir,
                &input_dir,
                Some(&Config {
                    templates: Some(templates.clone()),
                    template: Some(template.clone()),
                    ..Default::default()
                }),
            )?;

            for path in collect_files(&input_dir, &root_config, &mut glob_cache)? {
                let rel = path.strip_prefix(&input_dir)?;
                let mut out_path = output_dir.join(rel);

                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let config = match config::load_overrides(&path, &input_dir, Some(&root_config))
                    {
                        Ok(v) => v,
                        Err(e) => {
                            error!("failed to load config for {}: {}", path.display(), e);
                            Config::default()
                        }
                    };

                    let config_cloned = config.clone();
                    let final_templates = config_cloned.templates.unwrap_or(templates.clone());
                    let final_template = config_cloned.template.unwrap_or(template.clone());

                    let raw = match std::fs::read_to_string(&path) {
                        Ok(s) => s,
                        Err(e) => {
                            error!("failed to read {}: {}", path.display(), e);
                            continue;
                        }
                    };

                    let mut rendered =
                        match render_markdown(raw, &final_templates, &final_template, config).await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                error!("failed to render {}: {}", path.display(), e);
                                continue;
                            }
                        };

                    if let Some(true) = config_cloned.build.minify {
                        let mut cfg = Cfg::new();
                        cfg.enable_possibly_noncompliant();
                        cfg.minify_css = true;
                        cfg.minify_js = true;

                        let minified = minify(rendered.as_bytes(), &cfg);
                        rendered = match str::from_utf8(&minified) {
                            Ok(s) => s.to_string(),
                            Err(e) => {
                                error!("failed to minify {}: {}", path.display(), e);
                                continue;
                            }
                        };
                    }

                    out_path.set_extension("html");

                    if let Err(e) = std::fs::write(&out_path, rendered) {
                        error!("failed to write {}: {}", out_path.display(), e);
                    } else {
                        info!("wrote {}", out_path.display());
                    }
                } else {
                    std::fs::copy(&path, &out_path)?;
                    info!("copied {}", out_path.display());
                }
            }

            Ok(())
        }

        Commands::Serve {
            watch_dir,
            templates,
            template,
            host,
            port,
        } => {
            let livereload = LiveReloadLayer::new();
            let reloader = livereload.reloader();

            let state = WebState {
                templates: templates.clone(),
                template,
                watch_dir: watch_dir.clone(),
                glob_cache: Arc::new(Mutex::new(GlobCache::default())),
            };

            let app = Router::new()
                .route("/", get(|| async { Redirect::permanent("/index.html") }))
                .route("/{*path}", get(render_path_handler))
                .with_state(state)
                .layer(tower_http::trace::TraceLayer::new_for_http())
                .layer(livereload);

            let mut watcher = notify::recommended_watcher(move |ev: Result<_, _>| {
                if ev.is_ok_and(|evt: notify::Event| !evt.kind.is_access()) {
                    reloader.reload();
                }
            })?;
            watcher.watch(&watch_dir, notify::RecursiveMode::Recursive)?;
            watcher.watch(&templates, notify::RecursiveMode::Recursive)?;

            let addr = format!("{}:{}", host, port);
            info!("listening on http://{}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
            Ok(())
        }
    }
}

struct AppError(anyhow::Error);

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
