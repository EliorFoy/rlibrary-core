use crate::client::challenge::CHALLENGE_COOKIES;
use crate::client::sni::{self, ORIGIN_DOMAIN};
use crate::models::account::LoginResult;

/// 组装登录请求的 Cookie：全局挑战 cookie + 站点模式
fn build_cookie() -> String {
    let mut parts = Vec::new();
    parts.push(CHALLENGE_COOKIES.read().unwrap().cookie_str());
    parts.push("selectedSiteMode=books".to_string());
    parts.join("; ")
}


#[derive(serde::Serialize)]
#[allow(non_snake_case)]
struct LoginForm<'a> {
    isModal: &'a str,
    email: &'a str,
    password: &'a str,
    site_mode: &'a str,
    action: &'a str,
    redirectUrl: &'a str,
    gg_json_mode: &'a str,
}

/// 手动登录，成功后返回账号凭据（不落库，由调用方决定保存）
pub async fn manual_login(email: &str, password: &str) -> Result<LoginResult, String> {
    let url = format!("https://{ORIGIN_DOMAIN}/rpc.php");
    let origin = format!("https://{ORIGIN_DOMAIN}");
    let redirect_url = format!("https://{ORIGIN_DOMAIN}/");

    let form = LoginForm {
        isModal: "true",
        email,
        password,
        site_mode: "books",
        action: "login",
        redirectUrl: &redirect_url,
        gg_json_mode: "1",
    };

    let mut resp = sni::CLIENT
        .post(&url)
        .header_str("Host", ORIGIN_DOMAIN)
        .header_str("Cookie", &build_cookie())
        .header_str(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 Edg/145.0.0.0",
        )
        .header_str("Accept", "application/json, text/javascript, */*; q=0.01")
        .header_str("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6")
        .header_str("Origin", &origin)
        .header_str("Referer", &redirect_url)
        .header_str(
            "sec-ch-ua",
            r#""Not:A-Brand";v="99", "Microsoft Edge";v="145", "Chromium";v="145""#,
        )
        .header_str("sec-ch-ua-mobile", "?0")
        .header_str("sec-ch-ua-platform", r#""Windows""#)
        .header_str("cache-control", "no-cache")
        .header_str("pragma", "no-cache")
        .header_str("priority", "u=1, i")
        .header_str("sec-fetch-dest", "empty")
        .header_str("sec-fetch-mode", "cors")
        .header_str("sec-fetch-site", "same-origin")
        .header_str("x-requested-with", "XMLHttpRequest")
        .form(&form)
        .send()
        .await?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("登录失败 HTTP {status}: {body}"));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;

    let doc: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败: {e}"))?;

    let response = doc
        .get("response")
        .ok_or_else(|| format!("响应格式错误: {body}"))?;

    if response
        .get("validationError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        return Err(format!("登录失败: {msg}"));
    }

    let user_id = response
        .get("user_id")
        .and_then(|v| v.as_i64())
        .ok_or("响应中没有 user_id")?
        .to_string();
    let user_key = response
        .get("user_key")
        .and_then(|v| v.as_str())
        .ok_or("响应中没有 user_key")?
        .to_string();
    let name = response
        .get("user_name")
        .and_then(|v| v.as_str())
        .unwrap_or(email)
        .to_string();

    Ok(LoginResult {
        remix_userid: user_id,
        remix_userkey: user_key,
        username: name,
        email: email.to_string(),
        password: password.to_string(),
    })
}