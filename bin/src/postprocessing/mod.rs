use anyhow::Result;
use std::path::Path;

use crate::{config::RuntimeConfig, glob::GlobCache};

pub mod pipeline;
pub mod minify;

#[dynosaur::dynosaur(pub DynPostprocessor = dyn(box) Postprocessor)]
pub trait Postprocessor: Send + Sync {
    fn name(&self) -> &'static str;

    async fn process(
        &self,
        output_dir: &Path,
        config: &RuntimeConfig,
        glob_cache: &mut GlobCache,
    ) -> Result<()>;
}
