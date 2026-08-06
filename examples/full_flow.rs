use rlibrary_core::{
    account::pool,
    apis::{download, login, search},
    client::{ip, sni},
};

// ===========================================================================
// 完整流程示例：搜索 → 登录 → 解析 CDN 下载地址
//
// 运行：cargo run --example full_flow
//
// 说明：
//   - 底部可选登录邮箱/密码，填了就用账号身份解析（绕过每日下载限制）
//   - 不填则匿名解析（受 IP 每日 5 次下载限制）
// ===========================================================================

#[tokio::main]
async fn main() -> Result<(), String> {
    // ① 预热：解析真实 IP 并建立 SNI 连接
    let ip = ip::get_ip().await.map_err(|e| format!("IP 解析失败: {e}"))?;
    println!("[1] 使用 IP: {ip}");

    sni::CLIENT
        .get("https://z-library.sk/")
        .header_str("User-Agent", "Mozilla/5.0")
        .send()
        .await
        .map_err(|e| format!("首页预热失败: {e}"))?;
    println!("[2] SNI 客户端预热完成");

    // ② 可选：登录，凭据存入账号池
    let email = std::env::var("ZLIB_EMAIL").ok();
    let password = std::env::var("ZLIB_PASSWORD").ok();
    let account = match (email, password) {
        (Some(e), Some(p)) => {
            let cred = login::manual_login(&e, &p).await?;
            pool::add_account(cred.clone())?;
            println!("[3] 登录成功: id={}, name={}", cred.remix_userid, cred.username);
            Some((cred.remix_userid, cred.remix_userkey))
        }
        _ => {
            println!("[3] 未提供 ZLIB_EMAIL/ZLIB_PASSWORD，使用匿名身份");
            None
        }
    };

    // ③ 搜索
    let result = search::search_books("rust programming", 1).await?;
    println!(
        "[4] 搜索完成: 共 {total} 本, 第 {page}/{pages} 页",
        total = result.total,
        page = result.page,
        pages = result.total_pages
    );

    let book = result
        .books
        .first()
        .ok_or("搜索结果为空")?;
    println!(
        "[5] 选择书籍: 《{}》 by {} ({}, {})",
        book.title, book.author, book.extension, book.file_size
    );

    // ④ 解析最终 CDN 下载地址（不下载文件，只拿直链）
    let account_ref = account.as_ref().map(|(a, b)| (a.as_str(), b.as_str()));
    let cdn_url = download::resolve_download_url(book, account_ref).await?;
    println!("[6] 最终 CDN 下载地址:\n    {cdn_url}");

    println!("\n可将以上地址交给任意 HTTP 客户端下载（requests / DownloadManager 等）。");
    Ok(())
}
