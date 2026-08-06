use crate::account::pool;
use crate::client::challenge::CHALLENGE_COOKIES;
use crate::client::sni::CLIENT;
use crate::models::book::BookInfo;

// z-library 主域名
const ORIGIN_DOMAIN: &str = "z-library.sk";

// ===========================================================================
// 公开 API
// ===========================================================================

/// 解析一本书的最终 CDN 文件下载地址。
///
/// 内部用 SNI 客户端请求 z-library.sk 的下载链接，跟随重定向（302），
/// 最终返回 CDN 直链（如 `https://dln1.ncdn.ec/...`），可直接用
/// requests / Android DownloadManager 等任何 HTTP 客户端下载。
pub async fn resolve_download_url(
    book: &BookInfo,
    account: Option<(&str, &str)>,
) -> Result<String, String> {
    let url = &book.download_url;
    if url.is_empty() {
        return Err("下载链接为空".into());
    }

    // 1. 访问 z-library 原始下载链接，跟随重定向直到 200 或非 CDN 页面
    //    注意：只解析地址、不下载。凡指向 CDN 的 URL 直接返回，
    //    绝不向 CDN 发起请求（SNI 伪造会针对该 host，导致 TLS 失败）。
    let mut current = url.clone();
    let mut attempt = 0;
    let mut extra_cookie: Option<String> = None;

    loop {
        attempt += 1;

        // 若已是 CDN 地址，直接返回，不发起请求
        if let LocationKind::Cdn(cdn_host) = classify_location(&current) {
            eprintln!("[download] 最终 CDN 地址: {}", current);
            eprintln!("[download] CDN 主机: {}", cdn_host);
            return Ok(current);
        }

        eprintln!("[download] 请求 #{attempt}: {}", current);

        // 组装 cookie：全局挑战 + 账号凭据 + 站点模式 + 204 下发的 book_hash3_id
        let mut cookie = build_cookie(account);
        if let Some(extra) = &extra_cookie {
            cookie.push_str("; ");
            cookie.push_str(extra);
        }

        let resp = CLIENT
            .get(&current)
            .header_str("Cookie", &cookie)
            .max_redirect(0)
            .send()
            .await?;

        let status = resp.status().as_u16();
        eprintln!("[download] 响应状态: {} ({})", status, &current);

        // 此处 current 必为 origin 域名，跟随重定向或解析页面
        let step = follow_origin_response(resp, &current).await?;
        current = step.next_url;
        extra_cookie = step.extra_cookie;
        if attempt > 5 {
            return Err(format!("重定向超过 {attempt} 次仍停留在 origin 域名"));
        }
    }
}

// ===========================================================================
// 内部
// ===========================================================================

/// 判断 URL 所属类型
enum LocationKind {
    Origin,          // z-library.sk 自身域名
    Cdn(String),     // 第三方 CDN 域名，host 信息用于调试
}

fn classify_location(url: &str) -> LocationKind {
    let host = extract_host(url).unwrap_or_default();
    // 只凭 host 判断，不能用 substring：CDN 的 filename 里可能也带 "z-library.sk"
    if host == ORIGIN_DOMAIN {
        LocationKind::Origin
    } else {
        LocationKind::Cdn(host)
    }
}

fn extract_host(url: &str) -> Option<String> {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|u| u.split('/').next().map(String::from))
}

// 循环里用于把 204 下发的 book_hash3_id cookie 带到下一次请求
struct StepState {
    next_url: String,
    extra_cookie: Option<String>,
}

/// 处理 origin（z-library.sk）的响应，返回下一步信息
async fn follow_origin_response(
    mut resp: crate::client::sni::Response,
    request_url: &str,
) -> Result<StepState, String> {
    let status = resp.status().as_u16();

    if status == 204 {
        // 下载门登记：服务端下发 book_hash3_id cookie，需带上再请求一次
        let hash = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|s| s.starts_with("book_hash3_id"))
            .and_then(|s| s.split(';').next().map(String::from));
        return match hash {
            Some(h) => {
                eprintln!("[download] 204 下载门登记, book_hash3_id={}", h);
                Ok(StepState {
                    next_url: request_url.to_string(),
                    extra_cookie: Some(h),
                })
            }
            None => Err(format!("HTTP 204: 服务端未下发 book_hash3_id (URL={})", request_url)),
        };
    }

    if (300..400).contains(&status) {
        let location = resp
            .headers()
            .get("Location")
            .ok_or_else(|| format!("HTTP {status}: 重定向缺少 Location 头"))?
            .to_str()
            .map_err(|e| format!("Location 头解析失败: {e}"))?;

        let absolute = if location.starts_with("http") {
            location.to_string()
        } else {
            // 相对路径拼接
            let base = url::Url::parse(request_url)
                .map_err(|e| format!("解析请求 URL 失败: {e}"))?
                .join(location)
                .map_err(|e| format!("拼接相对 URL 失败: {e}"))?;
            base.to_string()
        };

        eprintln!("[download] 重定向 -> {}", absolute);
        Ok(StepState {
            next_url: absolute,
            extra_cookie: None,
        })
    } else if status == 200 {
        let body = resp.text().await?;
        if body.contains("Daily limit reached")
            || body.contains("download-limits-error")
        {
            return Err(
                "IP 已达每日下载限制（Daily limit reached），请使用账号下载：传入 account 参数或确保账号池 pool 中有可用账号"
                    .into(),
            );
        }
        let link = extract_download_link(&body)
            .ok_or_else(|| format!("HTTP 200 但无法从页面解析下载链接 (URL={})", request_url))?;
        eprintln!("[download] 从页面提取链接 -> {}", link);
        Ok(StepState {
            next_url: link,
            extra_cookie: None,
        })
    } else {
        Err(format!("origin 请求失败 HTTP {status}"))
    }
}

/// 从 HTML 页面中提取下载链接
fn extract_download_link(html: &str) -> Option<String> {
    let re = regex::Regex::new(
        r#"href="(https?://[^"]+)"[^>]*>\s*(?:Download|下载|GET|get)\b"#,
    )
    .ok()?;
    if let Some(cap) = re.captures(html) {
        return Some(cap[1].to_string());
    }
    let re2 = regex::Regex::new(
        r#"<a[^>]+href="(https?://[^"]+)"[^>]*class="[^"]*dlButton[^"]*""#,
    )
    .ok()?;
    if let Some(cap) = re2.captures(html) {
        return Some(cap[1].to_string());
    }
    None
}

/// 组装 Cookie：全局挑战 cookie + 账号池凭据
fn build_cookie(account: Option<(&str, &str)>) -> String {
    let mut parts = Vec::new();

    parts.push(CHALLENGE_COOKIES.read().unwrap().cookie_str());

    match account {
        Some((uid, ukey)) => {
            parts.push(format!("remix_userid={uid}"));
            parts.push(format!("remix_userkey={ukey}"));
        }
        None => {
            if let Some(acct) = pool::take_next() {
                parts.push(format!("remix_userid={}", acct.remix_userid));
                parts.push(format!("remix_userkey={}", acct.remix_userkey));
            }
        }
    }

    parts.push("selectedSiteMode=books".to_string());
    parts.join("; ")
}