//! C 语言 FFI 导出层（单独编译为 cdylib）。
//!
//! 供其它语言（C / C++ / Python ctypes / C# P/Invoke / Go cgo 等）通过
//! 共享库调用本 crate 的能力。所有导出函数均为**同步阻塞**式：
//! 内部维护一个全局 Tokio runtime，跨语言边界以 JSON 字符串交换数据。
//!
//! 返回约定：
//! - 成功：JSON `{"ok":true,...}`
//! - 失败：JSON `{"ok":false,"error":"..."}`
//! - 返回的字符串指针由调用方负责用 `rlibrary_free_string` 释放。
//!
//! # 构建
//! ```bash
//! cargo build --release
//! # 产物：target/release/rlibrary_core.dll / .so / .dylib
//! ```

use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// 全局 runtime
// ---------------------------------------------------------------------------

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("创建 Tokio runtime 失败"))
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

/// 统一调用入口：catch_unwind 防止 panic 跨 FFI 边界，结果序列化为 JSON。
fn run<F>(f: F) -> String
where
    F: FnOnce() -> Result<serde_json::Value, String> + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(Ok(v)) => serde_json::to_string(&v)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"序列化失败"}"#.into()),
        Ok(Err(e)) => serde_json::json!({"ok": false, "error": e}).to_string(),
        Err(_) => r#"{"ok":false,"error":"panic 已捕获"}"#.into(),
    }
}

/// 把 JSON 字符串序列化结果交给 C。
fn write_json(json: String) -> *mut c_char {
    CString::new(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// 读取 C 传入的可选字符串（null 视为 None）。
unsafe fn opt_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        None
    } else {
        // SAFETY: 调用方保证传入合法的以 NUL 结尾的 UTF-8 字符串
        Some(unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or(""))
    }
}

/// 读取 C 传入的必填字符串（null 视为空串）。
unsafe fn req_str<'a>(ptr: *const c_char) -> &'a str {
    // SAFETY: 调用方保证传入合法的以 NUL 结尾的 UTF-8 字符串
    unsafe { opt_str(ptr).unwrap_or("") }
}

// ===========================================================================
// 导出函数
// ===========================================================================

/// 搜索图书。
///
/// - `query`: 搜索关键词（UTF-8）
/// - `page`: 页码（从 1 开始）
/// - 返回 JSON：`{"ok":true,"total":N,"total_pages":N,"page":P,"books":[...]}`
///
/// # Safety
///
/// `query` 必须为 `NULL` 或指向 NUL 结尾的合法 UTF-8 字符串，且在调用期间保持有效。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rlibrary_search(query: *const c_char, page: u32) -> *mut c_char {
    let q = unsafe { req_str(query) }.to_string();
    let json = run(move || {
        let result = runtime().block_on(crate::apis::search::search_books(&q, page))?;
        let books: Vec<serde_json::Value> = result
            .books
            .iter()
            .map(|b| serde_json::to_value(b).unwrap_or_default())
            .collect();
        Ok(serde_json::json!({
            "ok": true,
            "total": result.total,
            "total_pages": result.total_pages,
            "page": result.page,
            "books": books,
        }))
    });
    write_json(json)
}

/// 登录 z-library。
///
/// - `email` / `password`: 账号凭据（UTF-8）
/// - 返回 JSON：`{"ok":true,"remix_userid":"...","remix_userkey":"...","username":"..."}`
///
/// # Safety
///
/// `email` / `password` 必须为 `NULL` 或指向 NUL 结尾的合法 UTF-8 字符串，且在调用期间保持有效。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rlibrary_login(email: *const c_char, password: *const c_char) -> *mut c_char {
    let email = unsafe { req_str(email) }.to_string();
    let password = unsafe { req_str(password) }.to_string();
    let json = run(move || {
        let cred =
            runtime().block_on(crate::apis::login::manual_login(&email, &password))?;
        Ok(serde_json::json!({
            "ok": true,
            "remix_userid": cred.remix_userid,
            "remix_userkey": cred.remix_userkey,
            "username": cred.username,
            "email": cred.email,
        }))
    });
    write_json(json)
}

/// 解析一本书的最终 CDN 下载地址（不下载文件）。
///
/// - `download_url`: 图书下载链接，如 `https://z-library.sk/dl/xxx`
/// - `user_id` / `user_key`: 可选账号凭据（可传 null 使用匿名/账号池）
/// - 返回 JSON：`{"ok":true,"url":"https://dln1.ncdn.ec/..."}`
///
/// # Safety
///
/// `download_url` 必须为指向 NUL 结尾的合法 UTF-8 字符串且在调用期间保持有效；
/// `user_id` / `user_key` 可为 `NULL`。所有指针在调用期间不得被释放或修改。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rlibrary_resolve_download_url(
    download_url: *const c_char,
    user_id: *const c_char,
    user_key: *const c_char,
) -> *mut c_char {
    let url = unsafe { req_str(download_url) }.to_string();
    let uid = unsafe { opt_str(user_id) }.map(String::from);
    let ukey = unsafe { opt_str(user_key) }.map(String::from);
    let json = run(move || {
        let account = match (&uid, &ukey) {
            (Some(u), Some(k)) => Some((u.as_str(), k.as_str())),
            _ => None,
        };
        let book = crate::models::book::BookInfo {
            download_url: url,
            ..Default::default()
        };
        let resolved = runtime().block_on(crate::apis::download::resolve_download_url(
            &book, account,
        ))?;
        Ok(serde_json::json!({ "ok": true, "url": resolved }))
    });
    write_json(json)
}

/// 释放由本库返回的字符串指针。
///
/// # Safety
///
/// `ptr` 必须是本库（`rlibrary_*` 函数）返回且未被释放过的指针；传 `NULL` 为无操作，
/// 但不得传其它来源的指针。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rlibrary_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(ptr);
    }
}