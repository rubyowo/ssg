use std::collections::HashMap;
use tera::Value;

use crate::PageContext;

use crate::plugin::GlobalPlugin;

pub struct TagAggregatorPlugin;

impl GlobalPlugin for TagAggregatorPlugin {
    fn name(&self) -> &str {
        "tag_aggregator"
    }

    fn run(
        &mut self,
        all_pages: &HashMap<String, PageContext>,
        global_context: &mut HashMap<String, tera::Value>,
    ) {
        let mut tag_counts: HashMap<String, usize> = HashMap::new();

        for (_path, page) in all_pages {
            let fm = &page.frontmatter;
            if fm.draft {
                continue;
            }

            if let Some(tags) = &fm.tags {
                for tag in tags {
                    *tag_counts.entry(tag.to_string()).or_insert(0) += 1;
                }
            }
        }

        if let Ok(tag_counts) = Value::try_from_serializable(&tag_counts) {
            global_context.insert("tags".to_string(), tag_counts);
        }
    }
}
