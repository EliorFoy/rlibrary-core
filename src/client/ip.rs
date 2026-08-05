use std::fs;
use std::net::Ipv4Addr;
use std::time::Duration;
use std::sync::{LazyLock, RwLock};
use regex::Regex;

const ORIGIN_DOMAIN: &str = "z-library.sk";
static CACHED_IP: LazyLock<RwLock<Option<Ipv4Addr>>> = LazyLock::new(|| RwLock::new(None));

// ---------------------------------------------------------------------------
// 持久化：文件读写
// ---------------------------------------------------------------------------
fn ip_cache_path() -> Option<std::path::PathBuf> {
    let dir = dirs::data_dir()?;
    Some(dir.join("rlibrary-core").join("ip_cache"))
}

fn load_cached_ip() -> Option<Ipv4Addr> {
    let path = ip_cache_path()?;
    let content = fs::read_to_string(&path).ok()?;
    content.trim().parse().ok()
}

fn save_ip_cache(ip: Ipv4Addr) {
    let Some(path) = ip_cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, ip.to_string());
}

// ---------------------------------------------------------------------------
// 公开 API
// ---------------------------------------------------------------------------

/// 获取 IP：优先文件缓存，没有则从 diggui 刷新
pub async fn get_ip() -> Result<Ipv4Addr, String> {
    // 先尝试从文件缓存中获取
    if let Some(ip) = load_cached_ip() {
        *CACHED_IP.write().unwrap() = Some(ip);
        return Ok(ip);
    }
    // 再尝试从内存缓存中获取
    if let Some(ip) = CACHED_IP.read().unwrap().as_ref() {
        return Ok(*ip);
    }
    // 最后从 diggui 刷新
    let ip = refresh_ip().await.ok_or("diggui 解析失败")?;
    *CACHED_IP.write().unwrap() = Some(ip);
    Ok(ip)
}

/// 从 diggui 刷新 IP 并写入文件缓存，失败时返回 Err
pub async fn refresh_ip() -> Result<Ipv4Addr, String> {
    let ip = resolve_from_diggui().await.ok_or("diggui 解析失败")?;
    save_ip_cache(ip);
    Ok(ip)
}

/// 通过 diggui.com 解析 ORIGIN_DOMAIN 的真实 IP
async fn resolve_from_diggui() -> Option<Ipv4Addr> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;

    let form = [
        ("type", "A"),
        ("hostname", ORIGIN_DOMAIN),
        ("nameserver", "public"),
        ("public", "8.8.8.8"),
        ("specify", ""),
        ("clientsubnet", ""),
        ("tcp", "def"),
        ("transport", "def"),
        ("mapped", "def"),
        ("nssearch", "def"),
        ("trace", "def"),
        ("recurse", "def"),
        ("edns", "def"),
        ("dnssec", "def"),
        ("subnet", "def"),
        ("cookie", "def"),
        ("all", "def"),
        ("cmd", "def"),
        ("question", "def"),
        ("answer", "def"),
        ("authority", "def"),
        ("additional", "def"),
        ("comments", "def"),
        ("stats", "def"),
        ("multiline", "def"),
        ("short", "def"),
        ("colorize", "on"),
    ];

    let resp = client
        .post("https://www.diggui.com/")
        .header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        )
        .form(&form)
        .send()
        .await
        .ok()?;

    let body = resp.text().await.ok()?;

    let re = Regex::new(&format!(
        r#"{}\.</a>\s+<span[^>]*>\d+</span>\s+<span[^>]*>IN</span>\s+<a[^>]*>A</a>\s+<a[^>]*>([0-9]{{1,3}}\.[0-9]{{1,3}}\.[0-9]{{1,3}}\.[0-9]{{1,3}})</a>"#,
        ORIGIN_DOMAIN.replace('.', "\\.")
    ))
    .ok()?;

    let ip = re.captures(&body)?.get(1)?.as_str().parse().ok()?;
    Some(ip)
}