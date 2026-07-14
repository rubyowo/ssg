use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use log::{debug, error, info, warn};
use minify_html::{Cfg, minify};
use notify::Watcher;
use rhai::Engine;
use std::{
    collections::HashMap, fs, path::{Path, PathBuf}, sync::Arc
};

use tokio::sync::Mutex;
use tower_livereload::LiveReloadLayer;

mod config;
mod glob;
mod rhai_plugin;

use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};

use lib::{
    PageContext, TemplateEngine, parse_md,
    plugin::{
        self, GlobalPipeline, NodeKind, PipelineBuiltinsExt, pipeline::PluginPipeline, syntax_highlighting::HighlighterThemeContext
    },
    render, render_template,
    tera::{self, Value},
    to_ast,
};

use crate::{
    config::Config,
    glob::GlobCache,
    rhai_plugin::{RhaiPlugin, register_rhai_filter, register_rhai_function},
};

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

pub fn generate_global_context(rendered_pages: &HashMap<String, PageContext>, config: &Config) -> anyhow::Result<HashMap<String, tera::Value>> {
    let mut global_pipeline = GlobalPipeline::new();
    global_pipeline.register(Box::new(plugin::tags::TagAggregatorPlugin));
    global_pipeline.register(Box::new(plugin::jsonfeed::JsonFeedPlugin::new(config.feed.clone())));
    
    let global_context = global_pipeline.run(&rendered_pages);
    Ok(global_context)
}

async fn render_markdown(content: String, config: &Config, rhai_engine: Arc<Engine>) -> Result<PageContext> {
    let (frontmatter, md_content) =
        parse_md(&content).with_context(|| "Failed to parse frontmatter")?;
    let mut ast = to_ast(md_content.to_string());

    let mut pipeline = PluginPipeline::new();

    let reading_time_ctx = Arc::new(std::sync::Mutex::new(
        plugin::reading_time::ReadingTimeContext::default(),
    ));
    let reading_ctx_clone = reading_time_ctx.clone();
    pipeline.register_native(NodeKind::Text, move |n| {
        if let Ok(mut ctx) = reading_ctx_clone.lock() {
            plugin::reading_time::reading_time_plugin(n, &mut ctx);
        }
    });

    let toc_ctx = Arc::new(std::sync::Mutex::new(plugin::toc::TocContext::default()));
    let toc_ctx_clone = toc_ctx.clone();
    pipeline.register_native(NodeKind::Heading, move |n| {
        if let Ok(mut ctx) = toc_ctx_clone.lock() {
            plugin::toc::toc_plugin(n, &mut ctx);
        }
    });

    pipeline.register_native(
        NodeKind::Code,
        plugin::syntax_highlighting::highlight_plugin,
    );
    pipeline.register_native(NodeKind::Code, plugin::math::math_plugin);
    pipeline.register_native(NodeKind::InlineMath, plugin::math::math_plugin);

    for script in &config.plugins {
        pipeline.register(RhaiPlugin::boxed(
            None,
            script.name.clone(),
            script.ast.clone(),
            rhai_engine.clone(),
        ));
    }

    pipeline.run_on(&mut ast);

    let final_reading_time = match Arc::try_unwrap(reading_time_ctx) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => arc.lock().map(|g| g.clone()).unwrap_or_default(),
    };

    let final_toc = match Arc::try_unwrap(toc_ctx) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => arc.lock().map(|g| g.clone()).unwrap_or_default(),
    };

    let content_html = render(&ast);
    Ok(PageContext {
        frontmatter,
        content: content_html,
        ast,
        reading: final_reading_time,
        toc: final_toc
    })
}

async fn render_tera(
    page_context: &PageContext,
    templates: &PathBuf,
    template_name: &str,
    config: Config,
    global_context: &HashMap<String, tera::Value>
) -> Result<String> {
    let themes: Vec<String> = config.clone().highlighting_themes.unwrap_or_else(|| {
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
    let css = base_css + themes_css.as_str();
    let hl_context = HighlighterThemeContext { themes, css };

    let custom_filters = config.filters.clone();
    let custom_functions = config.functions.clone();

    let rhai_engine = Arc::new(Engine::new());
    let engine_clone = rhai_engine.clone();
    let template_engine = TemplateEngine::new(templates, move |tera| {
        for filter in &custom_filters {
            register_rhai_filter(tera, engine_clone.clone(), filter);
        }

        for func in &custom_functions {
            register_rhai_function(tera, engine_clone.clone(), func);
        }

        Ok(())
    })?;

    let rendered = render_template!(
        template_engine,
        template_name,
        None,
        global => &global_context,
        page => &page_context,
        highlighter => &hl_context
    )
    .with_context(|| "Failed to render Tera template")?;

    Ok(rendered)
}

async fn render_or_copy_file(
    input_path: &Path,
    root_dir: &Path,
    output_dir: Option<&Path>,
    default_templates: &PathBuf,
    default_template: &str,
    config: &Config,
    page_context: Option<&PageContext>,
    global_context: &HashMap<String, tera::Value>,
    glob_cache: &mut GlobCache,
    for_build: bool,
) -> Result<Option<(Vec<u8>, String)>> {
    debug!("Processing input path: {}", input_path.display());
    if let Some(ignore) = config.build.ignore.as_ref() {
        if glob_cache.is_match(&ignore, input_path).unwrap_or(false) {
            debug!("Ignored path: {}", input_path.display());
            return Ok(None);
        }
    }

    let content = std::fs::read(input_path)?;
    let out_path = output_dir
        .map(|out| -> Result<PathBuf> {
            let relative = input_path.strip_prefix(root_dir).with_context(|| {
                format!(
                    "Failed to strip prefix {} from {}",
                    root_dir.display(),
                    input_path.display()
                )
            })?;
            Ok(out.join(relative))
        })
        .transpose()?;
    let is_markdown = input_path.extension().and_then(|s| s.to_str()) == Some("md");
    let mime = if is_markdown {
        "text/html".to_string()
    } else {
        mime_guess::from_path(&input_path)
            .first_or_octet_stream()
            .essence_str()
            .to_string()
    };

    if is_markdown {
        let templates = config
            .templates
            .as_ref()
            .unwrap_or_else(|| default_templates);
        let template = config.template.as_ref().map_or(default_template, |v| v);

        let ctx = match page_context {
            Some(c) => c,
            None => &{
                let rhai_engine = Arc::new(Engine::new());
                render_markdown(String::from_utf8_lossy(&content).to_string(), config, rhai_engine).await?
            }
        };

        let mut rendered = render_tera(
            ctx,
            templates,
            template,
            config.clone(),
            global_context,
        )
        .await?;

        if config.build.minify.unwrap_or(false) {
            let mut cfg = Cfg::new();
            cfg.enable_possibly_noncompliant();
            cfg.minify_css = true;
            cfg.minify_js = true;
            let minified = minify(rendered.as_bytes(), &cfg);
            rendered = String::from_utf8(minified)?;
        }

        if for_build {
            if let Some(mut out) = out_path {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                out.set_extension("html");
                std::fs::write(&out, &rendered)?;
                info!("Rendered {}", out.display());
            }
        }

        Ok(Some((rendered.into_bytes(), mime)))
    } else {
        if for_build {
            if let Some(out) = out_path {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(input_path, &out)?;
                info!("Copied {}", out.display());
            }
        }
        Ok(Some((content, mime)))
    }
}

#[axum::debug_handler]
async fn render_path_handler(
    State(state): State<WebState>,
    AxumPath(path): AxumPath<String>,
) -> Result<Response, AppError> {
    let mut requested = path.trim_matches('/').to_string();
    if requested.ends_with(".html") {
        requested = requested.trim_end_matches(".html").to_string();
        requested = format!("{}.md", requested);
    }

    let base = state.watch_dir.join(&requested);
    let md_path = if base.is_dir() {
        base.join("index.md")
    } else {
        base
    };

    let mut glob_cache = state.glob_cache.lock().await;

    let config = config::load_overrides(&md_path, &state.watch_dir, None).map_err(AppError)?;

    let mut rendered_pages = HashMap::new();
    let rhai_engine = Arc::new(Engine::new());
    let files = collect_files(&state.watch_dir, &config, &mut glob_cache)?;

    for f_path in files {
        if f_path.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Ok(content) = std::fs::read_to_string(&f_path) {
                if let Ok(page_ctx) = render_markdown(content, &config, rhai_engine.clone()).await {
                    let relative = f_path.strip_prefix(&state.watch_dir)?.to_string_lossy().to_string();
                    rendered_pages.insert(relative, page_ctx);
                }
            }
        }
    }

    let global_context = generate_global_context(&rendered_pages, &config)?;

    let relative_target = md_path.strip_prefix(&state.watch_dir)?.to_string_lossy().to_string();
    let target_context = rendered_pages.get(&relative_target);

    let (body, mime) = render_or_copy_file(
        &md_path,
        &state.watch_dir,
        None,
        &state.templates,
        &state.template,
        &config,
        target_context,
        &global_context,
        &mut glob_cache,
        false,
    )
    .await?
    .ok_or_else(|| AppError(anyhow::anyhow!("Ignored path: {}", md_path.display())))?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime)?);

    Ok((headers, Body::from(body)).into_response())
}

/// Recursively collect files, ignoring patterns
fn collect_files(root: &Path, config: &Config, glob_cache: &mut GlobCache) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if config.is_ignored(&dir, glob_cache) {
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
                    if config.is_ignored(&path, glob_cache) {
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

pub fn write_json_feed(
    output_dir: &Path,
    config: &Config,
    global_context: &std::collections::HashMap<String, Value>
) -> anyhow::Result<()> {
    if let Some(feed_value) = global_context.get("json_feed") {
        let filename = config.feed.as_ref()
            .and_then(|f| f.filename.clone())
            .unwrap_or_else(|| "feed.json".to_string());

        let target_path = output_dir.join(filename);
        
        let json_string = serde_json::to_string_pretty(feed_value)?;
        
        fs::write(&target_path, json_string)?;
        log::info!("Generated JSON Feed at {}", target_path.display());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
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

            let mut rendered_pages = HashMap::new();
            let rhai_engine = Arc::new(Engine::new());

            let files =collect_files(&input_dir, &root_config, &mut glob_cache)?;

            for path in &files {
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let content = std::fs::read_to_string(path)?;
                    let page_ctx = render_markdown(content, &root_config, rhai_engine.clone()).await?;
                    let relative_path = path.strip_prefix(&input_dir)?.to_string_lossy().to_string();
                    rendered_pages.insert(relative_path, page_ctx);
                }
            }
            
            let global_context = generate_global_context(&rendered_pages, &root_config)?;
            write_json_feed(&output_dir, &root_config, &global_context)?;
            
            for path in files {
                let relative_path = path.strip_prefix(&input_dir)?.to_string_lossy().to_string();
                let page_ctx = rendered_pages.get(&relative_path);

                let _ = render_or_copy_file(
                    &path,
                    &input_dir,
                    Some(&output_dir),
                    &templates,
                    &template,
                    &root_config,
                    page_ctx,
                    &global_context,
                    &mut glob_cache,
                    true,
                )
                .await?;
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
                .with_state(state.clone())
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

            let listener = tokio::net::TcpListener::bind(&addr).await?;
            Ok(axum::serve(listener, app).await?)
        }
    }
}

struct AppError(anyhow::Error);

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> Response {
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
