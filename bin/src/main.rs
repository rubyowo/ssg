use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{debug, error, info};
use notify::Watcher;
use rhai::Engine;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tower_http::services::ServeDir;

use tokio::sync::Mutex;
use tower_livereload::LiveReloadLayer;

mod config;
mod glob;
mod postprocessing;
mod render;
mod rhai_plugin;
mod utils;
mod web;

use axum::{Router, routing::get};

use lib::tera;

use crate::{
    config::Config,
    glob::GlobCache,
    postprocessing::pipeline::PostprocessPipeline,
    render::{create_tera, render_markdown, render_or_copy_file},
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
        #[arg(short, long)]
        template: Option<String>,
    },
    Serve {
        #[arg(default_value = ".")]
        watch_dir: PathBuf,
        template: Option<String>,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value = "3000")]
        port: u16,
    },
}

#[tokio::main]
#[hotpath::main]
async fn main() -> Result<()> {
    hotpath::tokio_runtime!();

    let args = Args::parse();
    tracing_subscriber::fmt::init();
    let mut glob_cache = GlobCache::default();

    match args.command {
        Commands::Build {
            input_dir,
            output_dir,
            template,
        } => {
            if !input_dir.is_dir() {
                error!("input path is not a directory: {}", input_dir.display());
                return Err(anyhow::anyhow!("input path is not a directory"));
            }
            tokio::fs::create_dir_all(&output_dir).await?;
            let config = config::load_overrides(
                &input_dir,
                &input_dir,
                None,
                Some(Config {
                    template,
                    ..Default::default()
                }),
            )?;

            info!("{:#?}", config);

            let mut rendered_pages = HashMap::new();
            let rhai_engine = Arc::new(Engine::new());

            let files = collect_files(&input_dir, &config, &mut glob_cache)?;

            let mut tasks = Vec::new();
            for path in &files {
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    let path = path.clone();
                    let input_dir = input_dir.clone();
                    let config = config.clone();
                    let rhai_engine = rhai_engine.clone();

                    tasks.push(tokio::spawn(async move {
                        let content = std::fs::read_to_string(&path)?;
                        let page_ctx = render_markdown(content, &config, rhai_engine).await?;
                        let relative_path =
                            path.strip_prefix(&input_dir)?.to_string_lossy().to_string();
                        anyhow::Ok((relative_path, page_ctx))
                    }));
                }
            }

            for handle in futures::future::join_all(tasks).await {
                match handle {
                    Ok(Ok((relative_path, page_ctx))) => {
                        rendered_pages.insert(relative_path, page_ctx);
                    }
                    Ok(Err(e)) => error!("Error parsing file: {e}"),
                    Err(e) => error!("Thread join error: {e}"),
                }
            }

            let global_context = generate_global_context(&rendered_pages, &config)?;
            write_json_feed(&output_dir, &config, &global_context)?;

            let template_engine =
                create_tera(&config, &rhai_engine).expect("Failed to create templating engine");

            for path in files {
                let relative_path = path.strip_prefix(&input_dir)?.to_string_lossy().to_string();
                let page_ctx = rendered_pages.get(&relative_path);

                let _ = render_or_copy_file(
                    &path,
                    &input_dir,
                    Some(&output_dir),
                    &template_engine,
                    &config,
                    page_ctx,
                    &global_context,
                    &mut glob_cache,
                    true,
                    rhai_engine.clone(),
                )
                .await?;
            }

            let pipeline = PostprocessPipeline::new(&config);
            pipeline.run(&output_dir, &config, &mut glob_cache).await?;

            Ok(())
        }

        Commands::Serve {
            watch_dir,
            template,
            host,
            port,
        } => {
            let livereload = LiveReloadLayer::new();
            let reloader = livereload.reloader();

            let config = config::load_overrides(
                &watch_dir,
                &watch_dir,
                None,
                Some(Config {
                    template,
                    ..Default::default()
                }),
            )?;

            let rhai_engine = Arc::new(Engine::new());
            let template_engine = Arc::new(tokio::sync::RwLock::new(create_tera(
                &config,
                &rhai_engine.clone(),
            )?));

            let state = WebState {
                template_engine,
                watch_dir: watch_dir.clone(),
                glob_cache: Arc::new(Mutex::new(GlobCache::default())),
                rendered_pages: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
                global_context: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
                rhai_engine,
            };

            let markdown_service = get(render_path_handler).with_state(state.clone());
            let static_service = ServeDir::new(&watch_dir).fallback(markdown_service);

            let app = Router::new()
                // .route("/", get(|| async { Redirect::permanent("/index.html") }))
                // .route("/{*path}", get(render_path_handler))
                .fallback_service(static_service)
                .with_state(state.clone())
                .layer(tower_http::trace::TraceLayer::new_for_http())
                .layer(livereload);

            let handle = tokio::runtime::Handle::current();
            let mut watcher = notify::recommended_watcher(move |ev: Result<notify::Event, _>| {
                if let Ok(ev) = ev
                    && !ev.kind.is_access()
                {
                    let mut state = state.clone();
                    let reloader_clone = reloader.clone();
                    handle.spawn(async move {
                            let abs_watch_dir = match std::fs::canonicalize(&state.watch_dir) {
                                Ok(path) => path,
                                Err(e) => {
                                    error!("Failed to canonicalize watch directory: {e}");
                                    return;
                                }
                            };

                            let mut needs_full_rebuild = false;
                            let mut to_recompile = vec![];

                            for path in ev.paths {
                                let abs_path = match std::fs::canonicalize(&path) {
                                    Ok(p) => p,
                                    Err(_) => path,
                                };

                                let ext = abs_path.extension().and_then(|s| s.to_str());
                                match ext {
                                    Some("md") => {
                                        to_recompile.push(abs_path);
                                    }
                                    Some("rhai") | Some("toml") | Some("html") | Some("tera") => {
                                        needs_full_rebuild = true;
                                        break;
                                    }
                                    _ => {}
                                }
                            }

                            if needs_full_rebuild {
                                info!("Plugin, config, or template changed. Rebuilding all pages in background");
                                if let Err(e) = state.rebuild_all().await {
                                    error!("Background full rebuild failed: {e}");
                                } else {
                                    info!("Background full rebuild completed successfully!");
                                }
                            } else {

                            for path in to_recompile {
                                if path.extension().and_then(|s| s.to_str()) == Some("md")
                                    && let Ok(content) = tokio::fs::read_to_string(&path).await {
                                        let root_config = match config::load_overrides(
                                            &path,
                                            &state.watch_dir,
                                            None, None
                                        ) {
                                            Ok(cfg) => cfg,
                                            Err(_) => continue,
                                        };
                                        debug!("Edited file content: {}", content.clone());
                                        match render_markdown(
                                            content,
                                            &root_config,
                                            state.rhai_engine.clone(),
                                        )
                                        .await
                                        {
                                            Ok(page_ctx) => {
                                                let relative = match path
                                                    .strip_prefix(&abs_watch_dir)
                                                {
                                                    Ok(rel) => rel.to_string_lossy().to_string(),
                                                    Err(e) => {
                                                        error!("Failed to strip prefix: {e}\nPrefix: {:#?}\nPath: {:#?}", state.watch_dir, path);
                                                        continue
                                                    },
                                                };

                                                let updated_global = {
                                                    let mut pages_write =
                                                        state.rendered_pages.write().await;
                                                    pages_write.insert(relative, page_ctx);
                                                    generate_global_context(
                                                        &pages_write,
                                                        &root_config,
                                                    )
                                                    .unwrap_or_default()
                                                };

                                                *state.global_context.write().await =
                                                    updated_global;
                                                info!("Background recompiled: {}", path.display());
                                            }
                                            Err(e) => {
                                                error!("Failed to render markdown in the background: {}", e);
                                            }
                                        }
                                    }
                            }
                        }

                            reloader_clone.reload();
                        });
                };
            })?;
            watcher.watch(&watch_dir, notify::RecursiveMode::Recursive)?;

            let addr = format!("{}:{}", host, port);
            info!("listening on http://{}", addr);

            let listener = tokio::net::TcpListener::bind(&addr).await?;
            Ok(axum::serve(listener, app).await?)
        }
    }
}
