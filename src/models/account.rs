
/// 登录成功后返回的账号凭据
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub remix_userid: String,
    pub remix_userkey: String,
    pub username: String,
    pub email: String,
    pub password: String,
}