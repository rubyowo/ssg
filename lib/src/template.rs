use anyhow::Ok;
use tera::{Context, Tera};

pub struct TemplateEngine {
    tera: Tera,
}

impl TemplateEngine {
    pub fn new<P: AsRef<std::path::Path>>(template_dir: P) -> anyhow::Result<Self> {
        let mut tera = Tera::new();
        tera.register_filter("format", tera_contrib::format::format);
        tera.register_function("now", tera_contrib::dates::now);
        tera.register_filter("date", tera_contrib::dates::date);
        tera.register_filter(
            "filesizeformat",
            tera_contrib::filesize_format::filesizeformat,
        );
        tera.register_filter("json_encode", tera_contrib::json::json_encode);
        tera.register_function("get_random", tera_contrib::rand::get_random);
        tera.register_filter("slug", tera_contrib::slug::slug);
        tera.register_filter("urlencode", tera_contrib::urlencode::urlencode);
        tera.register_filter(
            "urlencode_strict",
            tera_contrib::urlencode::urlencode_strict,
        );

        tera.load_from_glob(&format!("{}/**/*", template_dir.as_ref().to_str().unwrap()))?;
        tera.autoescape_on(vec![]);
        Ok(Self { tera })
    }

    pub fn render(&self, template: &str, ctx: Context) -> anyhow::Result<String> {
        Ok(self.tera.render(template, &ctx)?)
    }
}

/// Render a template using an existing global Tera context and plugin pipelines.
/// Usage:
/// ```rust
/// render!(
///     template_engine,
///     "test.tera",
///     Some(global_ctx), // or None
///     page => page_ctx,
///     reading => reading_pipeline.context,
///     toc => toc_pipeline.context,
/// );
/// ```
#[macro_export]
macro_rules! render_template {
    ($engine:expr, $template:expr, $global_ctx:expr $(, $key:ident => $val:expr )* $(,)? ) => {{
        let coerced_global_ctx: Option<&$crate::tera::Context> = $global_ctx;
        let mut ctx: $crate::tera::Context = match coerced_global_ctx {
          Some(global) => global.clone(),
          None => $crate::tera::Context::new()
        };

        $(
            ctx.insert(stringify!($key), &$val);
        )*

        $engine.render($template, ctx)
    }};
}
