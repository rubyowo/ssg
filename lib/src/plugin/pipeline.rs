use std::collections::HashMap;

use markdown::mdast::Node;

use crate::{
    PageContext,
    plugin::{GlobalPlugin, NativePlugin, NodeKind, children_mut},
};

pub struct GlobalPipeline {
    plugins: Vec<Box<dyn GlobalPlugin>>,
}

impl Default for GlobalPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalPipeline {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn GlobalPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn run(
        &mut self,
        all_pages: &HashMap<String, PageContext>,
    ) -> HashMap<String, tera::Value> {
        let mut global_context = HashMap::new();
        for plugin in &mut self.plugins {
            // println!("Running global plugin: {}", plugin.name());
            plugin.run(all_pages, &mut global_context);
        }
        global_context
    }
}

pub trait MarkdownPlugin: Send + Sync {
    /// Specify which Node type this plugin transforms.
    /// Returns `None` if it should evaluate against every single node.
    fn target_kind(&self) -> Option<NodeKind>;

    fn run(&mut self, node: &mut Node);
}

pub struct PluginPipeline {
    pub plugins: Vec<Box<dyn MarkdownPlugin>>,
}

impl Default for PluginPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginPipeline {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn MarkdownPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn run_on(&mut self, root: &mut Node) {
        fn walk(node: &mut Node, plugins: &mut [Box<dyn MarkdownPlugin>]) {
            if let Some(current_kind) = NodeKind::from_node(node) {
                for plugin in plugins.iter_mut() {
                    if plugin.target_kind().is_none_or(|k| k == current_kind) {
                        plugin.run(node);
                    }
                }
            }

            if let Some(children) = children_mut(node) {
                for child in children {
                    walk(child, plugins);
                }
            }
        }

        walk(root, &mut self.plugins);
    }
}

pub trait PipelineBuiltinsExt {
    fn register_native<F>(&mut self, kind: NodeKind, func: F)
    where
        F: FnMut(&mut Node) + Send + Sync + 'static;
}

impl PipelineBuiltinsExt for PluginPipeline {
    #[inline]
    fn register_native<F>(&mut self, kind: NodeKind, func: F)
    where
        F: FnMut(&mut Node) + Send + Sync + 'static,
    {
        self.register(NativePlugin::boxed(kind, func));
    }
}
