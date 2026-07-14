use anyhow::{Context, Ok};
use tera::Tera;

pub struct TemplateEngine {
    tera: Tera,
}

impl TemplateEngine {
    pub fn new<P, F>(template_dir: P, custom_config: F) -> anyhow::Result<Self>
    where
        P: AsRef<std::path::Path>,
        F: FnOnce(&mut Tera) -> anyhow::Result<()>,
    {
        let mut tera = Tera::default();
        tera.register_filter("format", tera_contrib::format::format);
        tera.register_function("now", tera_contrib::dates::now);
        tera.register_filter("date", tera_contrib::dates::date);
        tera.register_filter(
            "filesize_format",
            tera_contrib::filesize_format::filesize_format,
        );
        tera.register_filter("json_encode", tera_contrib::json::json_encode);
        tera.register_function("get_random", tera_contrib::rand::get_random);
        tera.register_filter("slug", tera_contrib::slug::slug);
        tera.register_filter("urlencode", tera_contrib::urlencode::urlencode);
        tera.register_filter(
            "urlencode_strict",
            tera_contrib::urlencode::urlencode_strict,
        );

        custom_config(&mut tera).with_context(|| "Failed running custom engine configuration")?;

        tera.load_from_glob(&format!("{}/**/*", template_dir.as_ref().to_str().unwrap()))?;
        tera.autoescape_on(Vec::<&str>::new());
        Ok(Self { tera })
    }

    pub fn render(&self, template: &str, ctx: tera::Context) -> anyhow::Result<String> {
        self.tera
            .render(template, &ctx)
            .with_context(|| format!("Failed to render template: {}", template))
    }
}

/// Render a template with plugin contexts.
/// Usage:
/// ```rust
/// render_template!(
///     template_engine,
///     "test.tera",
///     page => &page_ctx,
///     reading => &reading_ctx,
///     toc => &toc_ctx,
/// );
/// ```
#[macro_export]
macro_rules! render_template {
    ($engine:expr, $template:expr, $global_ctx:expr $(, $key:ident => $val:expr )* $(,)? ) => {{
        let mut ctx = $crate::tera::Context::new();
        $(
            ctx.insert(stringify!($key), $val);
        )*

        $engine.render($template, ctx)
    }};
}
