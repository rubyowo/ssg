use arborium::Highlighter;
use markdown::mdast::Node;

use crate::NodeExt;

pub fn highlight_plugin(node: &mut Node, _ctx: &mut ()) {
    if let Node::Code(code) = node {
        if let Some(lang) = code.lang.as_ref() {
            let mut hl = Highlighter::new();
            let html = format!(
                "<pre><code class=\"language-{}\">{}</code></pre>",
                lang,
                hl.highlight(lang, &code.value).unwrap()
            );

            node.replace_with_html(html);
        }
    }
}
