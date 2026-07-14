use crate::tera::Value;
use anyhow::Result;
use log::error;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::glob::GlobCache;
use lib::{
    PageContext,
    plugin::{self, GlobalPipeline},
};

/// Recursively collects files, skipping those that match patterns in the Config.
pub fn collect_files(
    root: &Path,
    config: &Config,
    glob_cache: &mut GlobCache,
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if config.is_ignored(&dir, glob_cache) {
            continue;
        }

        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                error!("failed to read dir {}: {}", dir.display(), e);
                continue;
            }
        };

        for entry in rd {
            match entry {
                Ok(ent) => {
                    let path = ent.path();
                    if config.is_ignored(&path, glob_cache) {
                        continue;
                    }
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        files.push(path);
                    }
                }
                Err(e) => {
                    error!("failed to read entry in {}: {}", dir.display(), e);
                }
            }
        }
    }
    Ok(files)
}

pub fn generate_global_context(
    rendered_pages: &HashMap<String, PageContext>,
    config: &Config,
) -> Result<HashMap<String, Value>> {
    let mut global_pipeline = GlobalPipeline::new();
    global_pipeline.register(Box::new(plugin::tags::TagAggregatorPlugin));
    global_pipeline.register(Box::new(plugin::jsonfeed::JsonFeedPlugin::new(
        config.feed.clone(),
    )));

    let global_context = global_pipeline.run(rendered_pages);
    Ok(global_context)
}

pub fn write_json_feed(
    output_dir: &Path,
    config: &Config,
    global_context: &HashMap<String, Value>,
) -> Result<()> {
    if let Some(feed_value) = global_context.get("json_feed") {
        let filename = config
            .feed
            .as_ref()
            .and_then(|f| f.filename.clone())
            .unwrap_or_else(|| "feed.json".to_string());

        let target_path = output_dir.join(filename);
        let json_string = serde_json::to_string_pretty(feed_value)?;

        fs::write(&target_path, json_string)?;
        log::info!("Generated JSON Feed at {}", target_path.display());
    }
    Ok(())
}
