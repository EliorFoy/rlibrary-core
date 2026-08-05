use sha1::{Digest, Sha1};

/// PoW 挑战结果
pub struct Challenge {
    pub token: String,
    pub check_offset: usize,
}

/// 从 HTML 解析挑战（混淆 JS 数组 + 洗牌）
pub fn parse_challenge(html: &str) -> Option<Challenge> {
    let token = extract_js_array(html, 2)?;
    if token.len() < 2 {
        return None;
    }
    let check_offset = usize::from_str_radix(&token[0..1], 16).ok()?;
    Some(Challenge { token, check_offset })
}

/// SHA1 暴力求解 PoW（寻找 i 使 digest[check_offset]==0xB0 且 digest[check_offset+1]==0x0B）
pub fn solve(challenge: &Challenge) -> u64 {
    let mut i: u64 = 0;
    loop {
        let input = format!("{}{}", challenge.token, i);
        let digest = Sha1::digest(input.as_bytes());
        if digest[challenge.check_offset] == 0xB0
            && digest[challenge.check_offset + 1] == 0x0B
        {
            return i;
        }
        i += 1;
    }
}

/// 构建挑战 cookie。
/// `account` 为 `(userid, userkey)`，传 `None` 则不填 remix 凭据。
pub fn build_challenge_cookie(
    challenge: &Challenge,
    solution: u64,
    elapsed_ms: u64,
    account: Option<(&str, &str)>,
) -> String {
    let mut parts = vec![format!(
        "c_token={}{}; c_time={:.3}",
        challenge.token,
        solution,
        elapsed_ms as f64 / 1000.0,
    )];
    if let Some((uid, ukey)) = account {
        parts.push(format!("remix_userid={uid}"));
        parts.push(format!("remix_userkey={ukey}"));
    }
    parts.push("selectedSiteMode=books".to_string());
    parts.join("; ")
}

// ---------------------------------------------------------------------------
// 内部：混淆 JS 数组解析
// ---------------------------------------------------------------------------

const ARRAY_MARKER: &str = "const a0_0x2a54=['";
const SHUFFLE_MARKER: &str = "_0x4457dc(++_0x2a548e);}(a0_0x2a54,";

fn extract_js_array(html: &str, index: usize) -> Option<String> {
    let start = html.find(ARRAY_MARKER)? + ARRAY_MARKER.len();
    let remaining = &html[start..];
    let array_end = remaining.find("'];")?;
    let array_str = &remaining[..array_end];

    let items: Vec<&str> = array_str
        .split("','")
        .map(|s| s.trim_matches('\''))
        .collect();

    if items.len() != 3 {
        return None;
    }

    let shuffle_count = parse_shuffle_count(&remaining[array_end..])?;
    let rotated_index = (index + shuffle_count) % 3;
    Some(items[rotated_index].to_string())
}

fn parse_shuffle_count(s: &str) -> Option<usize> {
    let pos = s.find(SHUFFLE_MARKER)? + SHUFFLE_MARKER.len();
    let hex_str = &s[pos..];
    let end = hex_str.find(')')?;
    let hex_val = &hex_str[..end].trim().trim_start_matches("0x");
    usize::from_str_radix(hex_val, 16).ok()
}