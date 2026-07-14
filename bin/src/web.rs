use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
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
    pub templates: PathBuf,
    pub template: String,
    pub template_engine: TemplateEngine,
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
    pub async fn rebuild_all(&self) -> Result<()> {
        let config = config::load_overrides(&self.watch_dir, &self.watch_dir, None)?;
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
    State(state): State<WebState>,
    uri: Uri
) -> Result<Response, AppError> {
    let path = percent_encoding::percent_decode_str(uri.path())
        .decode_utf8_lossy()
        .into_owned();
    let requested = path.trim_matches('/').to_string();

    let mut target_path = state.watch_dir.join(&requested);
    if target_path.is_dir() {
        target_path = target_path.join("index.md");
    }

    // if an html file is requested, check if the corresponding markdown file exists
    let md_path = if requested.ends_with(".html") {
        let md_equivalent = state
            .watch_dir
            .join(format!("{}.md", requested.trim_end_matches(".html")));

        if md_equivalent.is_file() {
            md_equivalent
        } else {
            target_path
        }
    } else {
        target_path
    };

    let is_markdown = md_path.extension().and_then(|s| s.to_str()) == Some("md") && md_path.is_file();
    if is_markdown {
        let cache_is_empty = state.rendered_pages.read().await.is_empty();
        if cache_is_empty {
            state.rebuild_all().await.map_err(AppError)?;
        }
    } else {
        return Err(AppError(anyhow::anyhow!("Not markdown: {}", md_path.display())));
    }

    let config = config::load_overrides(&md_path, &state.watch_dir, None).map_err(AppError)?;

    let relative_target = md_path
        .strip_prefix(&state.watch_dir)?
        .to_string_lossy()
        .to_string();

    let (target_context, global_read) = {
        let pages_read = state.rendered_pages.read().await;
        let global_read = state.global_context.read().await;

        (
            pages_read.get(&relative_target).cloned(),
            global_read.clone(),
        )
    };

    let mut glob_cache = state.glob_cache.lock().await;

    let (body, mime) = render_or_copy_file(
        &md_path,
        &state.watch_dir,
        None,
        &state.templates,
        &state.template,
        &state.template_engine,
        &config,
        target_context.as_ref(),
        &global_read,
        &mut glob_cache,
        false,
        state.rhai_engine,
    )
    .await?
    .ok_or_else(|| AppError(anyhow::anyhow!("Ignored path: {}", md_path.display())))?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime)?);

    Ok((headers, Body::from(body)).into_response())
}

pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
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
