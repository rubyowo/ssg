use markdown::{
    CompileOptions, Constructs, Options, ParseOptions, mdast::Node, to_html_with_options, to_mdast,
};
use md_crate::mdast::Html;
use mdast_util_to_markdown::to_markdown;

fn parse_options() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            math_flow: true,
            math_text: true,
            ..Constructs::gfm()
        },
        ..Default::default()
    }
}

fn options() -> Options {
    Options {
        parse: parse_options(),
        compile: CompileOptions {
            allow_dangerous_html: true,
            allow_dangerous_protocol: true,
            ..CompileOptions::gfm()
        },
    }
}

// these functions never error with normal markdown because markdown does not have syntax errors

pub fn to_ast(content: String) -> Node {
    to_mdast(&content, &parse_options()).unwrap()
}

pub fn render(ast: &Node) -> String {
    to_html_with_options(&from_ast(ast), &options()).unwrap()
}

// Okay this one will actually error but uh it's only turning the AST we give it back into markdown so I think it's fine?
pub fn from_ast(ast: &Node) -> String {
    to_markdown(ast).unwrap()
}

pub trait NodeExt {
    fn replace_with_html(&mut self, html: String);
}

impl NodeExt for Node {
    fn replace_with_html(&mut self, html: String) {
        *self = Node::Html(Html {
            value: html,
            position: None,
        });
    }
}
