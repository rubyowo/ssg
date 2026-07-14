use crate::{PageContext, plugin::GlobalPlugin};
use jfeed::{Author, Content, Dates, Feed, Item};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tera::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FeedConfig {
    pub title: Option<String>,
    pub home_page_url: String,
    pub feed_url: String,
    pub author_name: String,
    pub author_url: Option<String>,
    pub avatar_url: Option<String>,

    pub filename: Option<String>,
}

pub struct JsonFeedPlugin {
    config: Option<FeedConfig>,
}

impl JsonFeedPlugin {
    pub fn new(config: Option<FeedConfig>) -> Self {
        Self { config }
    }
}

impl GlobalPlugin for JsonFeedPlugin {
    fn name(&self) -> &str {
        "json_feed"
    }

    fn run(
        &mut self,
        all_pages: &HashMap<String, PageContext>,
        global_context: &mut HashMap<String, Value>,
    ) {
        let feed_cfg = match &self.config {
            Some(cfg) => cfg,
            None => return,
        };

        let mut feed_builder = Feed::builder();
        let mut feed_builder = feed_builder
            .set_home_page(&feed_cfg.home_page_url)
            .set_url(&feed_cfg.feed_url)
            .set_version(&jfeed::FeedVersion::JSONFeed1_1);

        if let Some(desc) = &feed_cfg.title {
            feed_builder = feed_builder.set_title(desc);
        } else {
            feed_builder = feed_builder.set_title(&feed_cfg.home_page_url);
        }

        let mut author = &mut Author::builder();
        author = author.set_name(&feed_cfg.author_name);
        if let Some(author_url) = &feed_cfg.author_url {
            author = author.set_url(author_url);
        }
        if let Some(avatar_url) = &feed_cfg.avatar_url {
            author = author.set_avatar_url(avatar_url);
        }
        let author = author.build().expect("JSON Feed needs at least one author");
        feed_builder = feed_builder.add_author(&author);

        let mut items = Vec::new();

        for (path, page) in all_pages {
            let fm = &page.frontmatter;

            if fm.draft {
                continue;
            }

            let web_path = if path.ends_with("index.md") {
                path.trim_end_matches("index.md").to_string()
            } else {
                path.replace(".md", ".html")
            };

            let id_and_url = format!(
                "{}/{}",
                feed_cfg.home_page_url.trim_end_matches('/'),
                web_path.trim_start_matches('/')
            );

            let mut item_builder = Item::builder();
            let mut item_builder = item_builder
                .set_id(&id_and_url)
                .set_url(&id_and_url)
                .set_title(&fm.title)
                .set_summary(&fm.description.clone().unwrap_or_default())
                .add_author(&author);

            let mut content = Content::builder();
            let content = content.set_html(&page.content);
            if let Ok(content) = content.build() {
                item_builder.set_content(&content);
            }

            if let Some(date) = fm.date {
                let mut dates_builder = Dates::builder();
                // Pass the compliant RFC 3339 string format variant
                dates_builder.set_published(&format!("{}T00:00:00Z", date));
                if let Ok(dates) = dates_builder.build() {
                    item_builder = item_builder.set_dates(&dates);
                }
            }

            if let Some(tags) = &fm.tags {
                for tag in tags {
                    item_builder = item_builder.add_tag(tag);
                }
            }

            match item_builder.build() {
                Ok(item) => {
                    items.push((fm.date, item));
                }
                Err(e) => {
                    eprintln!("Error while building JSON feed item: {}", e)
                }
            }
        }

        items.sort_by_key(|b| std::cmp::Reverse(b.0));

        for (_, item) in items {
            feed_builder = feed_builder.add_item(&item);
        }

        match feed_builder.build() {
            Ok(completed_feed) => {
                if let Ok(tera_value) = Value::try_from_serializable(&completed_feed) {
                    global_context.insert("json_feed".to_string(), tera_value);
                }
            }
            Err(e) => {
                eprintln!("Error while building JSON feed: {}", e)
            }
        }
    }
}
