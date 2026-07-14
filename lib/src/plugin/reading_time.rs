use markdown::mdast::Node;
use serde::Serialize;

#[derive(Clone, Copy, Default, Serialize, Debug)]
pub struct ReadingTimeContext {
    pub word_count: usize,
    pub reading_time_minutes: usize,
}

pub fn reading_time_plugin(node: &mut Node, ctx: &mut ReadingTimeContext) {
    if let Node::Text(t) = node {
        ctx.word_count += t.value.split_whitespace().count();
        ctx.reading_time_minutes = (ctx.word_count as f64 / 200.0).ceil() as usize;
    }
}
