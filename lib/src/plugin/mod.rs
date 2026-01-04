pub mod pipeline;
pub mod reading_time;
pub mod toc;
pub mod syntax_highlighting;

use markdown::mdast::Node;

pub use pipeline::*;

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
        Image,
        Html,
        Break,
        ThematicBreak
    ]
}

pub trait PluginContext {}

#[derive(Copy)]
pub struct Plugin<C> {
    pub kind: NodeKind,
    pub func: fn(&mut Node, &mut C),
}

impl<C> Clone for Plugin<C> {
    fn clone(&self) -> Self {
        Plugin {
            kind: self.kind,
            func: self.func,
        }
    }
}