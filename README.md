# rlibrary-core

Z-Library 的 Rust 核心库：基于 **SNI 伪造 + PoW 挑战求解** 的 HTTP 客户端，提供搜索、登录、解析 CDN 直链等能力。

## 特性

- **SNI 伪造客户端**：通过特定域名/IP 组合绕过网络封锁访问 z-library.sk
- **DiamWall PoW 挑战自动求解**：遇到 503 自动解析、求解 SHA1 挑战并重试（search / login / download 共享全局 challenge token）
- **IP 解析与缓存**：从 diggui 获取真实 IP，内存 + 文件双缓存
- **图书搜索**：解析搜索结果、总数、分页
- **账号登录与池**：登录 z-library，凭据持久化到 SQLite，下载请求自动轮换账号
- **CDN 直链解析**：跟随重定向获取最终文件下载地址（不实际下载，避免 SNI 作用于 CDN 导致 TLS 失败）

## 目录结构

```
src/
├── apis/
│   ├── search.rs    图书搜索
│   ├── login.rs     账号登录（返回 remix_userid/remix_userkey）
│   └── download.rs  解析 CDN 下载直链
├── client/
│   ├── sni.rs       SNI 伪造客户端 + 502/503 自动重试
│   ├── challenge.rs DiamWall PoW 挑战解析/求解/全局 cookie
│   └── ip.rs        真实 IP 解析与缓存
├── account/
│   └── pool.rs      账号池（内存 + SQLite 持久化，round-robin）
├── models/
│   ├── book.rs      BookInfo / SearchResult
│   └── account.rs   LoginResult
└── lib.rs
```

## 快速开始

```rust
use rlibrary_core::{
    apis::{download, search},
    client::ip,
};

#[tokio::main]
async fn main() -> Result<(), String> {
    // ① 预热（可选）：解析 IP 并建立 SNI 连接
    let _ = ip::get_ip().await?;

    // ② 搜索
    let result = search::search_books("rust programming", 1).await?;
    let book = &result.books[0];

    // ③ 解析最终 CDN 下载地址（匿名；受 IP 每日 5 次下载限制）
    let cdn_url = download::resolve_download_url(book, None).await?;
    println!("{cdn_url}");
    Ok(())
}
```

## 完整示例

```bash
# 匿名运行
cargo run --example full_flow

# 带账号运行（绕过每日下载限制）
ZLIB_EMAIL=you@example.com ZLIB_PASSWORD=yourpassword cargo run --example full_flow
```

## FFI / Python 调用

- [FFI.md](FFI.md) — C ABI 导出层用法（搜索 / 登录 / 解析直链），含 Python ctypes 完整示例
- 可直接运行 `target/release/call_rlibrary.py` 体验账号登录 → 搜索 → 解析 CDN 直链

## 开发计划

- 见 [TODO.md](TODO.md)：账号查询管理、注册接口补充等

## Cookie 依赖说明

系统内有两套 cookie，由 SNI 客户端统一管理：

| Cookie | 内容 | 存储 | 写入时机 | 使用方 |
|---|---|---|---|---|
| Challenge | `c_token` / `c_time` / `bsrv` | 内存（`CHALLENGE_COOKIES`） | 任一请求返回 **503** 时自动求解并更新 | search / login / download 主动携带 |
| Login | `remix_userid` / `remix_userkey` | 内存 + SQLite（账号池） | `manual_login` 成功后 | download 请求身份认证 |

- 所有走 `sni::CLIENT` 的请求（search / login / download / 预热）共享同一个全局 challenge token。
- 任何一方撞 503 都会自动求解并更新全局，重试时立即生效。
- search / login / download 都会主动携带全局 challenge cookie；download 额外携带登录凭据。

## 测试

```bash
# 匿名下载地址解析（受 IP 每日 5 次下载限制，可能返回 "Daily limit reached"）
cargo test --test test_download -- --nocapture

# 带账号下载地址解析（推荐）
cargo test --test test_download_account -- --nocapture
```

> `test_download_account.rs` 中登录凭据需自行替换为有效账号。

## 注意事项

- **每日下载限制**：匿名 IP 每天最多 5 次下载，超限后 `/dl/...` 返回 200 限制页。登录后同一 IP 可继续下载。
- **CDN 直链不下载**：`resolve_download_url` 只返回最终地址。对 CDN 域名发起请求会被 SNI 伪造影响导致 TLS 失败，因此不要在库内下载文件，交给业务侧处理。
- **数据存储位置**：IP 缓存与账号 DB 位于系统数据目录下的 `rlibrary-core/`。
