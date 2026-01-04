use markdown::mdast::Node;

use crate::plugin::{NodeKind, Plugin, children_mut};

pub struct PluginPipeline<C> {
    by_kind: Vec<Vec<Plugin<C>>>,
    pub context: C,
}

impl<C: Default> PluginPipeline<C> {
    pub fn new() -> Self {
        Self {
            by_kind: vec![Vec::new(); NodeKind::COUNT],
            context: C::default(),
        }
    }

    pub fn register(&mut self, plugin: Plugin<C>) {
        self.by_kind[plugin.kind as usize].push(plugin);
    }

    #[inline(always)]
    pub fn run(&mut self, kind: NodeKind, node: &mut Node) {
        for plugin in &self.by_kind[kind as usize] {
            (plugin.func)(node, &mut self.context);
        }
    }
}

pub trait PipelineTrait {
    fn run(&mut self, kind: NodeKind, node: &mut Node);
}

impl<C> PipelineTrait for PluginPipeline<C> {
    fn run(&mut self, kind: NodeKind, node: &mut Node) {
        for plugin in &self.by_kind[kind as usize] {
            (plugin.func)(node, &mut self.context);
        }
    }
}

pub fn run_pipelines(node: &mut Node, pipelines: &mut [&mut dyn PipelineTrait]) {
    fn walk_node(node: &mut Node, pipelines: &mut [&mut dyn PipelineTrait]) {
        if let Some(kind) = NodeKind::from_node(node) {
            for pipeline in pipelines.iter_mut() {
                pipeline.run(kind, node);
            }
        }

        if let Some(children) = children_mut(node) {
            for child in children {
                walk_node(child, pipelines);
            }
        }
    }

    walk_node(node, pipelines);
}

/// Macro to call `run_pipelines` with a list of pipelines and their contexts
/// Example usage:
/// ```rust
/// run_pipelines!(
///     &mut root_node,
///     pipeline1,
///     pipeline2,
/// );
/// ```
#[macro_export]
macro_rules! run_pipelines {
    ($node:expr, $( $pipeline:expr ),+ $(,)? ) => {{
        let mut pipelines: &mut [&mut dyn $crate::plugin::PipelineTrait] = &mut [
            $(
                &mut $pipeline,
            )+
        ];

        $crate::plugin::run_pipelines($node, pipelines)
    }};
}