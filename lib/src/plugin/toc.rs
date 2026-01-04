use markdown::mdast::Node;
use serde::Serialize;

use crate::plugin::PluginContext;

#[derive(Debug, Default, Clone, Serialize)]
pub struct TocContext {
    pub headings: Vec<TocItem>,
}

impl PluginContext for TocContext {}

#[derive(Debug, Clone, Serialize)]
pub struct TocItem {
    pub level: u8,
    pub text: String,
    pub id: String
}


pub fn toc_plugin(node: &mut Node, ctx: &mut TocContext) {
    if let Node::Heading(heading) = node {
        let mut text = String::new();

        // Flatten all text nodes inside this heading
        for child in heading.children.iter() {
            if let Node::Text(t) = child {
                text.push_str(&t.value);
            } else if let Node::Code(code) = child {
                text.push_str(&code.value);
            }
        }

        ctx.headings.push(TocItem {
            level: heading.depth as u8,
            text: text.clone(),
            id: text.to_lowercase().replace(" ", "-")
        });
    }
}