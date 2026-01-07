use anyhow::Context;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct GlobCache {
    /// Canonicalized pattern list → compiled GlobSet
    sets: HashMap<Box<[String]>, GlobSet>,
}

impl GlobCache {
    /// Compile (or fetch) a GlobSet for these patterns.
    ///
    /// Patterns are canonicalized (sorted, deduplicated) once.
    pub fn get(&mut self, patterns: &[String]) -> anyhow::Result<&GlobSet> {
        if patterns.is_empty() {
            static EMPTY: once_cell::sync::Lazy<GlobSet> =
                once_cell::sync::Lazy::new(|| GlobSetBuilder::new().build().unwrap());
            return Ok(&EMPTY);
        }

        let mut key: Vec<String> = patterns.to_vec();
        key.sort_unstable();
        key.dedup();

        let key: Box<[String]> = key.into_boxed_slice();

        if !self.sets.contains_key(&key) {
            let mut builder = GlobSetBuilder::new();
            for pat in key.iter() {
                builder.add(
                    Glob::new(pat)
                        .with_context(|| format!("invalid glob pattern: {pat}"))?,
                );
            }
            let set = builder.build()?;
            self.sets.insert(key.clone(), set);
        }

        Ok(self.sets.get(&key).unwrap())
    }

    #[inline]
    pub fn is_match(
        &mut self,
        patterns: &[String],
        path: &std::path::Path,
    ) -> anyhow::Result<bool> {
        Ok(self.get(patterns)?.is_match(path))
    }
}