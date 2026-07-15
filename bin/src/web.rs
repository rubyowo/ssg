use anyhow::Result;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use lib::{PageContext, TemplateEngine, tera};
use rhai::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::{
    config,
    glob::GlobCache,
    render::{render_markdown, render_or_copy_file},
    utils::{collect_files, generate_global_context},
};

#[derive(Clone)]
pub struct WebState {
    pub template_engine: Arc<RwLock<TemplateEngine>>,
    pub watch_dir: PathBuf,
    pub glob_cache: Arc<Mutex<GlobCache>>,
    pub rendered_pages: Arc<RwLock<HashMap<String, PageContext>>>,
    pub global_context: Arc<RwLock<HashMap<String, tera::Value>>>,
    pub rhai_engine: Arc<Engine>,
}

#[hotpath::measure_all]
impl WebState {
    /// Recompiles every markdown file in the watch directory concurrently
    /// and updates the shared memory caches.
    pub async fn rebuild_all(&mut self) -> Result<()> {
        {
            let mut engine_write = self.template_engine.write().await;
            // Reload all templates during a full rebuild
            engine_write.reload_templates()?;
        }

        let config = config::load_overrides(&self.watch_dir, &self.watch_dir, None, None)?;
        let files = {
            let mut glob_cache = self.glob_cache.lock().await;
            collect_files(&self.watch_dir, &config, &mut glob_cache)?
        };

        let mut initial_pages = HashMap::new();
        let mut tasks = vec![];

        for f_path in files {
            if f_path.extension().and_then(|s| s.to_str()) == Some("md") {
                let watch_dir = self.watch_dir.clone();
                let config = config.clone();
                let rhai_engine = self.rhai_engine.clone();

                tasks.push(tokio::spawn(async move {
                    if let Ok(content) = tokio::fs::read_to_string(&f_path).await
                        && let Ok(page_ctx) = render_markdown(content, &config, rhai_engine).await
                    {
                        let relative = f_path
                            .strip_prefix(&watch_dir)?
                            .to_string_lossy()
                            .to_string();
                        return anyhow::Ok(Some((relative, page_ctx)));
                    }
                    anyhow::Ok(None)
                }));
            }
        }

        for task in futures::future::join_all(tasks).await {
            if let Ok(Ok(Some((rel, ctx)))) = task {
                initial_pages.insert(rel, ctx);
            }
        }

        let initial_global = generate_global_context(&initial_pages, &config)?;

        *self.rendered_pages.write().await = initial_pages;
        *self.global_context.write().await = initial_global;

        Ok(())
    }
}

#[axum::debug_handler]
#[hotpath::measure]
pub async fn render_path_handler(
    State(mut state): State<WebState>,
    uri: Uri,
) -> Result<Response, AppError> {
    let path = percent_encoding::percent_decode_str(uri.path()).decode_utf8_lossy();
    let requested = match path.trim_matches('/') {
        "" => "index.html",
        other => other,
    };

    // Ensure caches are populated
    if state.rendered_pages.read().await.is_empty() {
        state.rebuild_all().await?;
    }

    let global_read = state.global_context.read().await;
    let engine = state.template_engine.read().await;

    let (source_path, target_context) = {
        // 1) Check if there exists a tera template with the same filename (with the .tera extension)
        // eg: "/archive.html" -> "archive.html.tera", "/about" -> "about.tera"
        let tera_target = format!("{requested}.tera");
        let tera_path = state.watch_dir.join(&tera_target);

        if tera_path.is_file() {
            (tera_path, None)
        } else {
            // 2) Otherwise, check if there exists a markdown file with the same filename (with .html replaced)
            // eg: "/posts/hello.html" -> "/posts/hello.md", "/about" -> "/about.md"
            let md_target = if requested.ends_with(".html") {
                format!("{}.md", requested.trim_end_matches(".html"))
            } else {
                requested.to_string()
            };
            let md_path = state.watch_dir.join(&md_target);

            if md_path.is_file() {
                let relative_target = md_path.strip_prefix(&state.watch_dir)?.to_string_lossy();
                let context = state
                    .rendered_pages
                    .read()
                    .await
                    .get(relative_target.as_ref())
                    .cloned();

                (md_path, context)
            } else {
                // 3) Otherwise, send a 404
                return Err(AppError::NotFound(format!("Page not found: /{requested}")));
            }
        }
    };

    let config = config::load_overrides(&source_path, &state.watch_dir, None, None)?;
    let mut glob_cache = state.glob_cache.lock().await;

    let (body, mime) = render_or_copy_file(
        &source_path,
        &state.watch_dir,
        None,
        &engine,
        &config,
        target_context.as_ref(),
        &global_read,
        &mut glob_cache,
        false,
        state.rhai_engine,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("Ignored path: {}", source_path.display()))?;

    Ok(create_response(Body::from(body), mime))
}

fn create_response(body: Body, mime: String) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(header_val) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, header_val);
    }
    (headers, body).into_response()
}

pub enum AppError {
    NotFound(String),
    Internal(anyhow::Error),
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self::Internal(err.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg).into_response(),
            AppError::Internal(err) => {
                log::error!("Internal server error: {:?}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Internal Server Error: {}", err),
                )
                    .into_response()
            }
        }
    }
}
