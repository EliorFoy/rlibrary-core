use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BookInfo {
    pub id: String,
    pub isbn: String,
    pub title: String,
    pub author: String,
    pub download_url: String,
    pub detail_url: String,
    pub publisher: String,
    pub language: String,
    pub year: String,
    pub extension: String,
    pub file_size: String,
    pub rating: String,
    pub quality: String,
    pub image_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub books: Vec<BookInfo>,
    pub total: u32,
    pub page: u32,
    pub total_pages: u32,
}