use std::fs;
use std::sync::{LazyLock, Mutex, RwLock};

use crate::models::account::LoginResult;
use rusqlite::{params, Connection};

// ===========================================================================
// DB 层
// ===========================================================================

fn db_path() -> std::path::PathBuf {
    let dir = dirs::data_dir().unwrap_or_else(|| ".".into());
    dir.join("rlibrary-core").join("accounts.db")
}

static DB: LazyLock<Mutex<Connection>> = LazyLock::new(|| {
    let path = db_path();
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let conn = Connection::open(&path).expect("打开 accounts.db 失败");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
            remix_userid  TEXT PRIMARY KEY,
            remix_userkey TEXT NOT NULL,
            username      TEXT NOT NULL,
            email         TEXT NOT NULL,
            password      TEXT NOT NULL
        );",
    )
    .expect("建表失败");
    Mutex::new(conn)
});

// ===========================================================================
// 内存层 + 读写 API
// ===========================================================================

static ACCOUNT_POOL: LazyLock<RwLock<Vec<LoginResult>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// 启动时从 DB 回填内存（只执行一次）
fn load_from_db() {
    let mut guard = ACCOUNT_POOL.write().unwrap();
    if !guard.is_empty() {
        return;
    }
    let conn = DB.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT remix_userid, remix_userkey, username, email, password FROM accounts")
        .expect("查询 accounts 表失败");
    let rows = stmt
        .query_map([], |r| {
            Ok(LoginResult {
                remix_userid: r.get(0)?,
                remix_userkey: r.get(1)?,
                username: r.get(2)?,
                email: r.get(3)?,
                password: r.get(4)?,
            })
        })
        .expect("读取失败");
    for row in rows.flatten() {
        guard.push(row);
    }
}

/// 添加一个账号：写内存 + 写 DB（持久化）
pub fn add_account(acct: LoginResult) -> Result<(), String> {
    // ① 写内存
    ACCOUNT_POOL.write().unwrap().push(acct.clone());

    // ② 同步写 DB（remix_userid 作为主键，重复插入则更新）
    DB.lock().unwrap()
        .execute(
            "INSERT OR REPLACE INTO accounts (remix_userid, remix_userkey, username, email, password)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![acct.remix_userid, acct.remix_userkey, acct.username, acct.email, acct.password],
        )
        .map_err(|e| format!("写入 DB 失败: {e}"))
        .map(|_| ())
}

/// 轮换取出下一个账号（FIFO round-robin）
///
/// 取出队头，放回队尾，返回副本供本次请求使用。
/// round-robin 是进程内状态，不写 DB。
pub fn take_next() -> Option<LoginResult> {
    // 内存空时尝试从 DB 加载
    if ACCOUNT_POOL.read().unwrap().is_empty() {
        load_from_db();
    }
    let mut g = ACCOUNT_POOL.write().unwrap();
    if g.is_empty() {
        return None;
    }
    let acct = g.remove(0);
    g.push(acct.clone());
    Some(acct)
}

/// 查询所有账号信息（只读）
pub fn list_accounts() -> Vec<LoginResult> {
    if ACCOUNT_POOL.read().unwrap().is_empty() {
        load_from_db();
    }
    ACCOUNT_POOL.read().unwrap().clone()
}

/// 按 user_id 精确取出并移除某个账号（从内存和 DB 同时删除）
pub fn remove_by_user_id(id: &str) -> Option<LoginResult> {
    let mut g = ACCOUNT_POOL.write().unwrap();
    let idx = g.iter().position(|a| a.remix_userid == id)?;
    let acct = g.remove(idx);
    // DB 同步删除
    DB.lock().unwrap()
        .execute("DELETE FROM accounts WHERE remix_userid = ?1", [id])
        .ok()?;
    Some(acct)
}

/// 当前池数量
pub fn account_count() -> usize {
    ACCOUNT_POOL.read().unwrap().len()
}