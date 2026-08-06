# C FFI 导出层（cdylib）

本 crate 编译类型为 `rlib + cdylib`，导出同步、阻塞式的 C ABI，供任何能调用共享库的语言使用：C / C++ / Python ctypes / C# P/Invoke / Go cgo 等。跨语言边界统一用 **JSON 字符串** 交换数据。

## 构建

```bash
cargo build --release
# 产物
#   Windows: target/release/rlibrary_core.dll
#   macOS:   target/release/librlibrary_core.dylib
#   Linux:   target/release/librlibrary_core.so
```

## 导出函数

| 函数 | 签名 | 说明 |
|---|---|---|
| `rlibrary_search` | `(const char* query, uint32_t page) -> char*` | 搜索图书 |
| `rlibrary_login` | `(const char* email, const char* password) -> char*` | 登录 z-library |
| `rlibrary_resolve_download_url` | `(const char* url, const char* user_id, const char* user_key) -> char*` | 解析最终 CDN 直链 |
| `rlibrary_free_string` | `(char* ptr)` | 释放返回值字符串 |

### 约定

- **同步阻塞**：内部维护全局 Tokio runtime `Runtime`（`OnceLock` 单例），函数内部 `block_on` 异步逻辑。可在任意线程调用。
- **返回约定**：
  - 成功：`{"ok": true, ...}`
  - 失败：`{"ok": false, "error": "..."}`（内部 `catch_unwind` 兜住 panic，不会跨 FFI 边界 `unwind`）
- **内存**：返回值是 `malloc` 出来的 NUL 结尾 UTF-8 字符串，**必须**由调用方用 `rlibrary_free_string` 释放，否则泄漏。
- **可选参数**（如 `rlibrary_resolve_download_url` 的 `user_id`/`user_key`）：传 `NULL` 表示使用匿名 / 账号池。

## 函数详情

### `rlibrary_search(query, page)`

按关键词搜索，`page` 从 1 开始。

成功返回：

```json
{
  "ok": true,
  "total": 51,
  "total_pages": 51,
  "page": 1,
  "books": [
    {
      "id": "48251014",
      "isbn": "",
      "title": "PYTHON: Learn Python Programming in 90 minutes or Less! ...",
      "author": "AZ Elite Publishing",
      "publisher": "",
      "language": "english",
      "year": "",
      "extension": "pdf",
      "file_size": "799 KB",
      "rating": "",
      "quality": "",
      "image_url": "",
      "detail_url": "https://z-library.sk/book/QO54zAL1vr/...html",
      "download_url": "https://z-library.sk/dl/GZO9JmOaBA"
    }
  ]
}
```

> **注意空格编码**：`query` 里的空格会被编码为 `%20`（path 段语义）。Z-Library 会把字面 `+` 当作搜索词的连接符，
> 若误用 `+` 会产生名如 `python+programming` 的**占位假条目**（书名=作者=关键词），其 `/dl/...` 永远返回 204、无法下载。

### `rlibrary_login(email, password)`

成功返回：

```json
{
  "ok": true,
  "remix_userid": "35246529",
  "remix_userkey": "...",
  "username": "1700118540@qq.com",
  "email": "1700118540@qq.com"
}
```

失败（如密码错误）返回 `{"ok": false, "error": "Incorrect email or password"}`。

### `rlibrary_resolve_download_url(url, user_id, user_key)`

`url` 取自搜索结果的 `download_url`（形如 `https://z-library.sk/dl/xxx`）。`user_id`/`user_key` 来自
`rlibrary_login` 返回值，可传 `NULL` 走匿名 / 账号池。

失败（如每日下载限制）返回：
`{"ok": false, "error": "HTTP 200: Daily limit reached（本书需账号登录或已达每日下载限制）"}`。

## Python 示例（ctypes）

```python
import ctypes
import json

lib = ctypes.CDLL(str(DLL_PATH))

# 返回 char* 的函数：restype 必须设成 c_void_p，再手动 cast 解引用并 free，
# 否则堆损坏（退出码 0xC0000374）。
lib.rlibrary_search.restype = ctypes.c_void_p
lib.rlibrary_search.argtypes = [ctypes.c_char_p, ctypes.c_uint32]
lib.rlibrary_login.restype = ctypes.c_void_p
lib.rlibrary_login.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
lib.rlibrary_resolve_download_url.restype = ctypes.c_void_p
lib.rlibrary_resolve_download_url.argtypes = [
    ctypes.c_char_p, ctypes.c_char_p, ctypes.c_char_p,
]
lib.rlibrary_free_string.restype = None
lib.rlibrary_free_string.argtypes = [ctypes.c_void_p]


def call(lib, name, *args):
    fn = getattr(lib, name)
    raw = fn(*args)
    if not raw:
        return {"ok": False, "error": "null 指针"}
    try:
        text = ctypes.cast(raw, ctypes.c_char_p).value.decode("utf-8", "replace")
        return json.loads(text)
    finally:
        lib.rlibrary_free_string(ctypes.c_void_p(raw))


# 1) 登录
login = call(lib, "rlibrary_login", b"user@example.com", b"password")
assert login["ok"]
uid, ukey = login["remix_userid"].encode(), login["remix_userkey"].encode()

# 2) 搜索
res = call(lib, "rlibrary_search", b"rust programming", 1)
book = res["books"][0]

# 3) 解析 CDN 直链（带账号）
dl = call(lib, "rlibrary_resolve_download_url", book["download_url"].encode(), uid, ukey)
assert dl["ok"]
print(dl["url"])
```

> 仓库内已有可运行脚本：`target/release/call_rlibrary.py`（`uv run python call_rlibrary.py "python programming"`），
> 账号可通过环境变量 `ZLIB_EMAIL` / `ZLIB_PASSWORD` 覆盖。