use super::{Postprocessor, minify::Minifier};
use crate::glob::GlobCache;
use crate::{config::RuntimeConfig, postprocessing::DynPostprocessor};
use anyhow::{Context, Result};
use std::path::Path;

pub struct PostprocessPipeline<'a> {
    processors: Vec<Box<DynPostprocessor<'a>>>,
}

impl<'a> PostprocessPipeline<'a> {
    pub fn new(config: &RuntimeConfig) -> Self {
        let mut processors: Vec<Box<DynPostprocessor>> = Vec::new();

        if config.build.minify.unwrap_or(false) {
            processors.push(DynPostprocessor::new_box(Minifier));
        }

        Self { processors }
    }

    pub async fn run(
        &self,
        output_dir: &Path,
        config: &RuntimeConfig,
        glob_cache: &mut GlobCache,
    ) -> Result<()> {
        if self.processors.is_empty() {
            return Ok(());
        }

        println!("Running post-processing pipeline...");

        let mut postprocess_config = config.clone();

        // Remove any ignores pointing to the output directory
        if let Some(ref mut ignores) = postprocess_config.build.ignore {
            let output_dir_str = output_dir.to_string_lossy();
            ignores.retain(|pattern| !pattern.contains(&*output_dir_str));
        }

        for processor in &self.processors {
            println!("  ↳ Running {}...", processor.name());
            processor
                .process(output_dir, &postprocess_config, glob_cache)
                .await
                .with_context(|| format!("Post-processor '{}' failed", processor.name()))?;
        }

        Ok(())
    }
}
