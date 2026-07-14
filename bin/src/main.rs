use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{error, info};
use notify::Watcher;
use rhai::Engine;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

use tokio::sync::Mutex;
use tower_livereload::LiveReloadLayer;

mod config;
mod glob;
mod render;
mod rhai_plugin;
mod utils;
mod web;

use axum::{Router, response::Redirect, routing::get};

use lib::tera;

use crate::{
    config::Config,
    glob::GlobCache,
    render::{render_markdown, render_or_copy_file},
    utils::{collect_files, generate_global_context, write_json_feed},
    web::{WebState, render_path_handler},
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

            let files = collect_files(&input_dir, &root_config, &mut glob_cache)?;

            for path in &files {
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let content = std::fs::read_to_string(path)?;
                    let page_ctx =
                        render_markdown(content, &root_config, rhai_engine.clone()).await?;
                    let relative_path =
                        path.strip_prefix(&input_dir)?.to_string_lossy().to_string();
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
