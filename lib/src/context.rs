use serde::Serialize;
use markdown::mdast::Node;

use crate::frontmatter::Frontmatter;

#[derive(Serialize)]
pub struct PageContext {
    pub frontmatter: Frontmatter,
    pub content: String, 
    pub ast: Node,
}