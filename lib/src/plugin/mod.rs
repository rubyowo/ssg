pub mod math;
pub mod pipeline;
pub mod reading_time;
pub mod syntax_highlighting;
pub mod toc;
pub mod tags;
pub mod jsonfeed;

use std::collections::HashMap;

use markdown::mdast::Node;

pub use pipeline::*;

use crate::PageContext;

/// A plugin that processes the aggregated frontmatter of the entire site
pub trait GlobalPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn run(&mut self, all_pages: &HashMap<String, PageContext>, global_context: &mut HashMap<String, tera::Value>);
}

macro_rules! define_mdast_nodes {
    (
        children: [$($KindC:ident),* $(,)?],
        leaf: [$($KindL:ident),* $(,)?]
    ) => {
        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
        pub enum NodeKind {
            $($KindC),*,
            $($KindL),*
        }

        impl NodeKind {
            pub const COUNT: usize = 0 $(+ {let _ = stringify!($KindC); 1})* $(+ {let _ = stringify!($KindL); 1})*;

            #[inline(always)]
            pub fn from_node(node: &markdown::mdast::Node) -> Option<Self> {
                Some(match node {
                    $(
                        markdown::mdast::Node::$KindC(_) => NodeKind::$KindC,
                    )*
                    $(
                        markdown::mdast::Node::$KindL(_) => NodeKind::$KindL,
                    )*
                    _ => return None,
                })
            }
        }

        #[inline(always)]
        pub fn children_mut(node: &mut markdown::mdast::Node) -> Option<&mut Vec<markdown::mdast::Node>> {
            match node {
                $(
                    markdown::mdast::Node::$KindC(n) => Some(&mut n.children),
                )*
                $(
                    markdown::mdast::Node::$KindL(_) => None,
                )*
                _ => None,
            }
        }
    }
}

define_mdast_nodes! {
    children: [
        Root,
        Heading,
        Paragraph,
        Blockquote,
        List,
        ListItem,
        Emphasis,
        Strong,
        Link,
    ],
    leaf: [
        Text,
        Code,
        InlineCode,
        InlineMath,
        Image,
        Html,
        Break,
        ThematicBreak
    ]
}

pub struct NativePlugin<F> {
    kind: NodeKind,
    func: F,
}

impl<F> NativePlugin<F>
where
    F: FnMut(&mut Node) + Send + Sync,
{
    pub fn boxed(kind: NodeKind, func: F) -> Box<dyn MarkdownPlugin> 
    where
        Self: 'static
    {
        Box::new(Self { kind, func })
    }
}

impl<F> MarkdownPlugin for NativePlugin<F>
where
    F: FnMut(&mut Node) + Send + Sync,
{
    fn target_kind(&self) -> Option<NodeKind> {
        Some(self.kind)
    }

    fn run(&mut self, node: &mut Node) {
        (self.func)(node);
    }
}