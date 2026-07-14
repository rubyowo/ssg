use std::{path::Path, sync::Arc};

use lib::{md_crate::mdast::Node, plugin::{MarkdownPlugin, NodeKind}, tera::{self, Kwargs, TeraResult, Value}};
use rhai::{AST, Dynamic, Engine, Scope, serde::{from_dynamic, to_dynamic}};

use crate::config::RhaiScript;

pub fn compile_rhai_dir(dir_str: Option<String>, base_path: &Path) -> Vec<RhaiScript> {
    let mut scripts = Vec::new();
    let Some(path_str) = dir_str else {
        return scripts;
    };
    let path = base_path.join(Path::new(&path_str));

    if let Ok(entries) = std::fs::read_dir(path) {
        let engine = rhai::Engine::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().map_or(false, |ext| ext == "rhai") {
                if let Some(name) = p.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(ast) = engine.compile_file(p.clone()) {
                        scripts.push(RhaiScript {
                            name: name.to_string(),
                            ast,
                        });
                    }
                }
            }
        }
    }
    scripts
}

pub fn register_rhai_filter(tera: &mut tera::Tera, engine: Arc<Engine>, script: &RhaiScript) {
    let ast = script.ast.clone();
    let name = script.name.clone();

    tera.register_filter(
        name.clone(),
        move |val: Value, kwargs: Kwargs, _state: &tera::State| -> TeraResult<Value> {
            let r_val = to_dynamic(&val).map_err(|e| tera::Error::message(e.to_string()))?;

            let mut kwargs_map = std::collections::HashMap::new();
            for (k, v) in kwargs.iter() {
                kwargs_map.insert(k.to_string(), v.clone());
            }

            let r_kwargs =
                to_dynamic(&kwargs_map).map_err(|e| tera::Error::message(e.to_string()))?;

            let res: Dynamic = engine
                .call_fn(&mut Scope::new(), &ast, &name, (r_val, r_kwargs))
                .map_err(|e| tera::Error::message(e.to_string()))?;

            let out: toml::Value =
                from_dynamic(&res).map_err(|e| tera::Error::message(e.to_string()))?;
            Value::try_from_serializable(&out)
        },
    );
}

pub fn register_rhai_function(tera: &mut tera::Tera, engine: Arc<Engine>, script: &RhaiScript) {
    let ast = script.ast.clone();
    let name = script.name.clone();

    tera.register_function(
        name.clone(),
        move |kwargs: Kwargs, _state: &tera::State| -> TeraResult<Value> {
            let mut kwargs_map = std::collections::HashMap::new();
            for (k, v) in kwargs.iter() {
                kwargs_map.insert(k.to_string(), v.clone());
            }

            let r_kwargs =
                to_dynamic(&kwargs_map).map_err(|e| tera::Error::message(e.to_string()))?;

            let res: Dynamic = engine
                .call_fn(&mut Scope::new(), &ast, &name, (r_kwargs,))
                .map_err(|e| tera::Error::message(e.to_string()))?;

            let out: toml::Value =
                from_dynamic(&res).map_err(|e| tera::Error::message(e.to_string()))?;
            Value::try_from_serializable(&out)
        },
    );
}

pub struct RhaiPlugin {
    kind: Option<NodeKind>,
    name: String,
    ast: AST,
    engine: Arc<Engine>,
}

impl RhaiPlugin {
    pub fn boxed(kind: Option<NodeKind>, name: String, ast: AST, engine: Arc<Engine>) -> Box<dyn MarkdownPlugin> {
        Box::new(Self { kind, name, ast, engine })
    }
}

impl MarkdownPlugin for RhaiPlugin {
    fn target_kind(&self) -> Option<NodeKind> {
        self.kind
    }

    fn run(&mut self, node: &mut Node) {
        let kind_str = self.kind.map(|k| format!("{:?}", k)).unwrap_or_else(|| "Unknown".to_string());
        
        if let Ok(r_node) = to_dynamic(&node) {
            if let Ok(updated) = self.engine.call_fn::<Dynamic>(
                &mut Scope::new(),
                &self.ast,
                "execute",
                (r_node, kind_str)
            ) {
                if let Ok(actual_node) = from_dynamic(&updated) {
                    *node = actual_node;
                }
            }
        }
    }
}