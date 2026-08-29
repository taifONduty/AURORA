use serde::{Deserialize, Serialize};

use crate::TavilyFailure;

#[derive(Serialize)]
pub(super) struct SearchRequest<'a> {
    query: &'a str,
    search_depth: &'static str,
    max_results: u8,
    include_answer: bool,
    include_raw_content: &'static str,
    auto_parameters: bool,
}

impl<'a> SearchRequest<'a> {
    pub(super) const fn for_objective(query: &'a str) -> Self {
        Self {
            query,
            search_depth: "basic",
            max_results: 3,
            include_answer: false,
            include_raw_content: "text",
            auto_parameters: false,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct SearchResponse {
    pub(super) results: Vec<SearchResult>,
}

impl SearchResponse {
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, TavilyFailure> {
        serde_json::from_slice(bytes).map_err(|_| TavilyFailure::MalformedResponse)
    }
}

#[derive(Deserialize)]
pub(super) struct SearchResult {
    pub(super) title: Option<String>,
    pub(super) url: Option<String>,
    pub(super) raw_content: Option<String>,
}
