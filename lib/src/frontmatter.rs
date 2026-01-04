use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Frontmatter {
    pub title: String,
    pub date: Option<jiff::civil::Date>,
    pub tags: Option<Vec<String>>,
    pub draft: bool,
}

pub fn parse_md(content: &str) -> anyhow::Result<(Frontmatter, &str)> {
    Ok(markdown_frontmatter::parse::<Frontmatter>(content)?)
}