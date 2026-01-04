use markdown::mdast::Node;
use math_core::{LatexToMathML, MathCoreConfig};

use crate::NodeExt;

pub fn math_plugin(node: &mut Node, _ctx: &mut ()) {
    if let Node::Code(code) = node
        && let Some(lang) = &code.lang
        && lang == "math"
    {
        if let Ok(latex) = LatexToMathML::new(&MathCoreConfig {
            pretty_print: math_core::PrettyPrint::Always,
            ..Default::default()
        }) {
            if let Ok(mathml) =
                latex.convert_with_local_counter(&code.value, math_core::MathDisplay::Block)
            {
                let html = format!("<pre><code class=\"language-math\">{}</code></pre>", mathml);

                node.replace_with_html(html);
            }
        }
    } else if let Node::InlineMath(math) = node {
        let latex = LatexToMathML::const_default();
        if let Ok(mathml) =
            latex.convert_with_local_counter(&math.value, math_core::MathDisplay::Inline)
        {
            let html = format!("<code class=\"language-math math-inline\">{}</code>", mathml);

            node.replace_with_html(html);
        }
    }
}
