use crate::client::sni::{CLIENT, ORIGIN_DOMAIN};
use crate::models::book::{BookInfo, SearchResult};

/// 组装搜索请求的 Cookie：全局挑战 cookie + 站点模式
fn build_cookie() -> String {
    let mut parts = Vec::new();
    parts.push(crate::client::challenge::CHALLENGE_COOKIES.read().unwrap().cookie_str());
    parts.push("selectedSiteMode=books".to_string());
    parts.join("; ")
}

/// 搜索图书，返回搜索结果
pub async fn search_books(query: &str, page: u32) -> Result<SearchResult, String> {
    let encoded = urlencoding(query);
    let url = format!("https://{ORIGIN_DOMAIN}/s/{encoded}?page={page}").to_string();

    let mut resp = CLIENT
        .get(&url)
        .header_str("Cookie", &build_cookie())
        .send()
        .await?;
    let html = resp.text().await.map_err(|e| format!("读取失败: {e}"))?;

    let books = parse_books(&html);
    let total = parse_total_results(&html).unwrap_or(books.len() as u32);
    let total_pages = parse_total_pages(&html).unwrap_or(1).max(1);

    Ok(SearchResult {
        books,
        total,
        page,
        total_pages,
    })
}

fn parse_total_results(html: &str) -> Option<u32> {
    let re = regex::Regex::new(
        r#"data-type="book"[^>]*>[^<]*Books[^(]*\(([0-9]+)\)"#,
    )
    .ok()?;
    let cap = re.captures(html)?;
    cap[1].parse().ok()
}

fn parse_total_pages(html: &str) -> Option<u32> {
    let re = regex::Regex::new(r#"pagesTotal:\s*(\d+)"#).ok()?;
    let cap = re.captures(html)?;
    cap[1].parse().ok()
}

fn parse_books(html: &str) -> Vec<BookInfo> {
    let mut books = Vec::new();

    let card_re = regex::Regex::new(
        r#"<z-bookcard\s+(?P<attrs>[^>]*?)>(?P<content>[\s\S]*?)</z-bookcard>"#,
    )
    .unwrap();

    let attr_re = regex::Regex::new(r#"(\w+)\s*=\s*"([^"]*)""#).unwrap();
    let title_re =
        regex::Regex::new(r#"<div\s+slot="title">([\s\S]*?)</div>"#).unwrap();
    let author_re =
        regex::Regex::new(r#"<div\s+slot="author">([\s\S]*?)</div>"#).unwrap();
    let img_re =
        regex::Regex::new(r#"<img\s+[^>]*?data-src="([^"]*)""#).unwrap();

    for cap in card_re.captures_iter(html) {
        let attrs = &cap["attrs"];
        let content = &cap["content"];
        let mut book = BookInfo::default();

        for attr_cap in attr_re.captures_iter(attrs) {
            let key = &attr_cap[1];
            let val = attr_cap[2].to_string();
            match key {
                "id" => book.id = val,
                "isbn" => book.isbn = val,
                "href" => {
                    book.detail_url =
                        format!("https://{}{}", crate::client::sni::ORIGIN_DOMAIN, val);
                }
                "download" => {
                    book.download_url =
                        format!("https://{}{}", crate::client::sni::ORIGIN_DOMAIN, val);
                }
                "publisher" => book.publisher = val,
                "language" => book.language = val,
                "year" => book.year = val,
                "extension" => book.extension = val,
                "filesize" => book.file_size = val,
                "rating" => book.rating = val,
                "quality" => book.quality = val,
                _ => {}
            }
        }

        if let Some(t) = title_re.captures(content) {
            book.title = html_unescape(&t[1]);
        }
        if let Some(a) = author_re.captures(content) {
            book.author = html_unescape(&a[1]);
        }
        if let Some(i) = img_re.captures(content) {
            book.image_url = i[1].to_string();
        }

        books.push(book);
    }

    books
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

fn urlencoding(s: &str) -> String {
    // path 段空格的语义是 %20，而非表单的 +（Z-Library 会把 + 当作字面搜索词的连接符，
    // 否则会产生 "python+programming" 这种占位匹配）。
    s.chars()
        .map(|c| {
            if c == ' ' {
                "%20".to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}