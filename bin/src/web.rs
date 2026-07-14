use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rhai::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    pub watch_dir: PathBuf,
    pub glob_cache: Arc<Mutex<GlobCache>>,
}

#[axum::debug_handler]
pub async fn render_path_handler(
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
                    let relative = f_path
                        .strip_prefix(&state.watch_dir)?
                        .to_string_lossy()
                        .to_string();
                    rendered_pages.insert(relative, page_ctx);
                }
            }
        }
    }

    let global_context = generate_global_context(&rendered_pages, &config)?;
    let relative_target = md_path
        .strip_prefix(&state.watch_dir)?
        .to_string_lossy()
        .to_string();
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
