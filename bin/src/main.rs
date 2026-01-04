use clap::{Parser, Subcommand, builder::Str};
use env_logger;
use log::{error, info};
use notify::Watcher;
use std::path::PathBuf;
use tower_livereload::LiveReloadLayer;

use axum::Router;
use tokio::sync::broadcast;

use lib::{
    PageContext, TemplateEngine, parse_md,
    plugin::{
        NodeKind, Plugin,
        pipeline::PluginPipeline,
        reading_time::{ReadingTimeContext, reading_time_plugin},
        syntax_highlighting::highlight_plugin,
        toc::{TocContext, toc_plugin},
    },
    render, render_template, run_pipelines, to_ast,
};

#[derive(Parser)]
#[command(about = "Static site tools")]
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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

            let template_engine = TemplateEngine::new(&templates)?;

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

                let (frontmatter, content) = match parse_md(&raw) {
                    Ok(t) => t,
                    Err(e) => {
                        error!("failed to parse frontmatter for {}: {}", path.display(), e);
                        continue;
                    }
                };

                let mut ast = to_ast(content.to_string());

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

                run_pipelines!(
                    &mut ast,
                    reading_pipeline,
                    toc_pipeline,
                    highlighting_pipeline
                );

                let content_html = render(&ast);

                let page_ctx = PageContext {
                    frontmatter,
                    content: content_html.clone(),
                    ast: ast.clone(),
                };

                let rendered = match render_template!(template_engine, &template, None, page => page_ctx, reading => reading_pipeline.context, toc => toc_pipeline.context)
                {
                    Ok(s) => s,
                    Err(e) => {
                        error!("template render error for {}: {}", path.display(), e);
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
            let app = Router::new().layer(livereload);

            let mut watcher = notify::recommended_watcher(move |ev: Result<_, _>| {
                if ev.is_ok_and(|evt: notify::Event| !evt.kind.is_access()) {
                    reloader.reload();
                }
            })?;
            watcher.watch(&watch_dir, notify::RecursiveMode::Recursive)?;

            let addr = format!("{}:{}", host, port);
            info!("listening on http://{}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
            Ok(())
        }
    }
}
