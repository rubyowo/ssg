use markdown::mdast::Node;
use serde::Serialize;

use crate::frontmatter::Frontmatter;

#[derive(Clone, Debug, Serialize)]
pub struct PageContext {
    pub frontmatter: Frontmatter,
    pub content: String,
    #[serde(skip_serializing)]
    pub ast: Node,

    // plugin contexts
    pub reading: crate::plugin::reading_time::ReadingTimeContext,
    pub toc: crate::plugin::toc::TocContext,
}
