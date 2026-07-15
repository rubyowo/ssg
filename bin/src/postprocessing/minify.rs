use crate::{config::RuntimeConfig, glob::GlobCache, utils::collect_files};

use super::Postprocessor;
use anyhow::Result;
use log::debug;
use std::path::Path;

pub struct Minifier;

impl Postprocessor for Minifier {
    fn name(&self) -> &'static str {
        "Minifier"
    }

    async fn process(
        &self,
        output_dir: &Path,
        config: &RuntimeConfig,
        glob_cache: &mut GlobCache,
    ) -> Result<()> {
        let files = collect_files(output_dir, config, glob_cache)?;

        for path in files {
            debug!("collected file: {}", path.display());
            if path.is_file() {
                let ext = path.extension().and_then(|s| s.to_str());
                if ext == Some("html") || ext == Some("css") || ext == Some("js") {
                    let content = tokio::fs::read(&path).await?;

                    let mut cfg = minify_html::Cfg::new();
                    cfg.minify_css = true;
                    cfg.minify_js = true;

                    let minified = minify_html::minify(&content, &cfg);
                    tokio::fs::write(&path, minified).await?;
                }
            }
        }
        Ok(())
    }
}
