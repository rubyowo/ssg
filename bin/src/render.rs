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
    config::Config,
    glob::GlobCache,
    rhai_plugin::{RhaiPlugin, register_rhai_filter, register_rhai_function},
};

pub async fn render_markdown(
    content: String,
    config: &Config,
    rhai_engine: Arc<Engine>,
) -> Result<PageContext> {
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
        toc: final_toc,
    })
}

pub async fn render_tera(
    page_context: &PageContext,
    templates: &PathBuf,
    template_name: &str,
    config: Config,
    global_context: &HashMap<String, tera::Value>,
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

pub async fn render_or_copy_file(
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
                render_markdown(
                    String::from_utf8_lossy(&content).to_string(),
                    config,
                    rhai_engine,
                )
                .await?
            },
        };

        let mut rendered =
            render_tera(ctx, templates, template, config.clone(), global_context).await?;

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
