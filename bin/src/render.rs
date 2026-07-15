use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use lib::{
    PageContext, TemplateEngine, parse_md,
    plugin::{
        self, NodeKind, PipelineBuiltinsExt, pipeline::PluginPipeline,
        syntax_highlighting::HighlighterThemeContext,
    },
    render, render_template, tera, to_ast,
};
use log::{debug, info, warn};
use minify_html::{Cfg, minify};
use rhai::Engine;

use crate::{
    config::RuntimeConfig,
    glob::GlobCache,
    rhai_plugin::{RhaiPlugin, register_rhai_filter, register_rhai_function},
};

#[hotpath::measure]
pub async fn render_markdown(
    content: String,
    config: &RuntimeConfig,
    rhai_engine: Arc<Engine>,
) -> Result<PageContext> {
    let (frontmatter, md_content) =
        parse_md(&content).with_context(|| "Failed to parse frontmatter")?;
    debug!("Rendering content: {}", md_content);
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
            script.ast.clone(),
            rhai_engine.clone(),
        ));
    }

    pipeline.run_on(&mut ast);

    let final_reading_time = match Arc::try_unwrap(reading_time_ctx) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => arc.lock().map(|g| *g).unwrap_or_default(),
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
        toc: final_toc,
    })
}

#[hotpath::measure]
pub fn create_tera(
    config: &RuntimeConfig,
    rhai_engine: &Arc<Engine>,
) -> anyhow::Result<TemplateEngine> {
    let custom_filters = config.filters.clone();
    let custom_functions = config.functions.clone();

    let engine_clone = rhai_engine.clone();
    TemplateEngine::new(&config.layouts, move |tera| {
        for filter in &custom_filters {
            register_rhai_filter(tera, engine_clone.clone(), filter);
        }

        for func in &custom_functions {
            register_rhai_function(tera, engine_clone.clone(), func);
        }

        Ok(())
    })
}

#[hotpath::measure]
pub async fn render_tera(
    page_context: Option<&PageContext>,
    template_engine: &TemplateEngine,
    template_name: &str,
    config: &RuntimeConfig,
    global_context: &HashMap<String, tera::Value>,
) -> Result<String> {
    let themes: Vec<String> = config
        .highlighting_themes
        .as_deref()
        .unwrap_or(&[
            "catppuccin_mocha".to_string(),
            "catppuccin_latte".to_string(),
        ])
        .to_vec();

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

#[hotpath::measure]
pub async fn render_or_copy_file(
    input_path: &Path,
    root_dir: &Path,
    output_dir: Option<&Path>,
    template_engine: &TemplateEngine,
    config: &RuntimeConfig,
    page_context: Option<&PageContext>,
    global_context: &HashMap<String, tera::Value>,
    glob_cache: &mut GlobCache,
    for_build: bool,
    rhai_engine: Arc<Engine>,
) -> Result<Option<(Vec<u8>, String)>> {
    debug!("Processing input path: {}", input_path.display());
    if let Some(ignore) = config.build.ignore.as_ref()
        && glob_cache.is_match(ignore, input_path).unwrap_or(false)
    {
        debug!("Ignored path: {}", input_path.display());
        return Ok(None);
    }

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
    let extension = input_path.extension().and_then(|s| s.to_str());
    let mime = if extension == Some("md") {
        "text/html".to_string()
    } else {
        // Strip .tera to guess MIME of target
        let guess_path = if extension == Some("tera") {
            input_path.with_extension("")
        } else {
            input_path.to_path_buf()
        };

        mime_guess::from_path(guess_path)
            .first_or_octet_stream()
            .essence_str()
            .to_string()
    };

    match extension {
        Some("md") => {
            let template = &config.template;

            let ctx = match page_context {
                Some(c) => c,
                None => {
                    let content = tokio::fs::read_to_string(input_path).await?;
                    &render_markdown(content, config, rhai_engine.clone()).await?
                }
            };

            let mut rendered =
                render_tera(Some(ctx), template_engine, template, config, global_context).await?;

            if for_build {
                if config.build.minify.unwrap_or(false) {
                    let mut cfg = Cfg::new();
                    cfg.enable_possibly_noncompliant();
                    cfg.minify_css = true;
                    cfg.minify_js = true;
                    let minified = minify(rendered.as_bytes(), &cfg);
                    rendered = String::from_utf8(minified)?;
                }

                if let Some(mut out) = out_path {
                    if let Some(parent) = out.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    out.set_extension("html");
                    tokio::fs::write(&out, &rendered).await?;
                    info!("Rendered {}", out.display());
                }
            }

            Ok(Some((rendered.into_bytes(), mime)))
        }
        Some("tera") => {
            let template_name = input_path
                .strip_prefix(root_dir)
                .with_context(|| {
                    format!(
                        "Failed to resolve template name from path: {}",
                        input_path.display()
                    )
                })?
                .to_string_lossy()
                .to_string();

            let mut rendered = render_tera(
                None,
                template_engine,
                &template_name,
                config,
                global_context,
            )
            .await?;

            if for_build {
                if config.build.minify.unwrap_or(false) && mime == "text/html" {
                    let mut cfg = Cfg::new();
                    cfg.enable_possibly_noncompliant();
                    cfg.minify_css = true;
                    cfg.minify_js = true;
                    let minified = minify(rendered.as_bytes(), &cfg);
                    rendered = String::from_utf8(minified)?;
                }

                if let Some(mut out) = out_path {
                    if let Some(parent) = out.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }

                    let out_filename = out.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if let Some(stripped_name) =
                        out_filename.strip_suffix(".tera").map(|s| s.to_string())
                    {
                        out.set_file_name(stripped_name);
                    }

                    tokio::fs::write(&out, &rendered).await?;
                    info!("Rendered {}", out.display());
                }
            }

            Ok(Some((rendered.into_bytes(), mime)))
        }
        _ => {
            let content = tokio::fs::read(input_path).await?;
            if for_build && let Some(out) = out_path {
                if let Some(parent) = out.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(input_path, &out).await?;
                info!("Copied {}", out.display());
            }
            Ok(Some((content, mime)))
        }
    }
}
