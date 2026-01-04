use clap::{Parser, Subcommand};
use log::{error, info};
use notify::Watcher;
use std::path::PathBuf;
use tower_livereload::LiveReloadLayer;

use axum::{
    Router,
    extract::Path,
    http::StatusCode,
    response::{Html, Redirect},
    routing::get,
};

use lib::{
    PageContext, TemplateEngine, parse_md,
    plugin::{
        NodeKind, Plugin,
        math::math_plugin,
        pipeline::PluginPipeline,
        reading_time::{ReadingTimeContext, reading_time_plugin},
        syntax_highlighting::highlight_plugin,
        toc::{TocContext, toc_plugin},
    },
    render, render_template, run_pipelines, to_ast,
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

        #[arg(short, long, default_value = "test.tera")]
        template: String,
    },

    Serve {
        #[arg(default_value = ".")]
        watch_dir: PathBuf,

        #[arg(short = 'T', long, default_value = "templates")]
        templates: PathBuf,

        #[arg(short, long, default_value = "test.tera")]
        template: String,

        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        #[arg(long, default_value = "3000")]
        port: u16,
    },
}

async fn render_markdown(
    content: String,
    templates: &PathBuf,
    template_name: &str,
) -> anyhow::Result<String> {
    let (frontmatter, md_content) = parse_md(&content)?;
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

    let template_engine = TemplateEngine::new(templates)?;

    let rendered = render_template!(template_engine, template_name, None, page => page_ctx, reading => reading_pipeline.context, toc => toc_pipeline.context)?;

    Ok(rendered)
}

pub async fn render_path_handler(
    path: String,
    templates: PathBuf,
    template: String,
    watch_dir: PathBuf,
) -> Result<Html<String>, (StatusCode, String)> {
    let mut requested = path.trim_matches('/').to_string();

    if requested.ends_with(".html") {
        let base_name = requested.trim_end_matches(".html");
        requested = format!("{}.md", base_name);
    }

    let base = if requested.is_empty() {
        watch_dir.clone()
    } else {
        watch_dir.join(&requested)
    };

    let content = if base.is_dir() {
        let idx_md = base.join("index.md");
        if idx_md.is_file() {
            std::fs::read_to_string(&idx_md)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("index not found in directory: {}", base.display()),
            ))
        }
    } else if base.is_file() {
        std::fs::read_to_string(&base)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("file not found: {}", requested),
        ))
    }
    .map_err(|e| (StatusCode::NOT_FOUND, format!("Failed to read file: {}", e)))?;

    let rendered = render_markdown(content, &templates, &template)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render: {}", e),
            )
        })?;

    Ok(Html(rendered))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt::init();

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

            for entry in std::fs::read_dir(&input_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    continue;
                }

                let filename = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                let raw = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("failed to read {}: {}", path.display(), e);
                        continue;
                    }
                };

                let rendered = match render_markdown(raw, &templates, &template).await {
                    Ok(s) => s,
                    Err(e) => {
                        error!("failed to render {}: {}", path.display(), e);
                        continue;
                    }
                };

                let out_name = format!("{}.html", filename);
                let out_path = output_dir.join(out_name);

                if let Err(e) = std::fs::write(&out_path, rendered) {
                    error!("failed to write {}: {}", out_path.display(), e);
                } else {
                    info!("wrote {}", out_path.display());
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

            let watch_dir_route = watch_dir.clone();
            let templates_route = templates.clone();

            let app = Router::new()
                .route("/", get(|| async { Redirect::permanent("/index.html") }))
                .route(
                    "/{*path}",
                    get(async move |Path(path): Path<String>| {
                        render_path_handler(path, templates_route, template, watch_dir_route).await
                    }),
                )
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
