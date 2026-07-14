use arborium::Highlighter;
use markdown::mdast::Node;
use serde::Serialize;

use crate::NodeExt;

pub fn highlight_plugin(node: &mut Node) {
    if let Node::Code(code) = node {
        if let Some(lang) = code.lang.as_ref() {
            let mut hl = Highlighter::new();
            if let Some(_) = hl.store().get(lang)
                && lang != "math"
            {
                let html = format!(
                    "<pre><code class=\"language-{}\">{}</code></pre>",
                    lang,
                    hl.highlight(lang, &code.value).unwrap()
                );

                node.replace_with_html(html);
            }
        }
    }
}

#[derive(Serialize, Debug)]
pub struct HighlighterThemeContext {
    pub themes: Vec<String>,
    pub css: String,
}
