use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use crate::client::challenge::CHALLENGE_COOKIES;
use http_body_util::BodyExt as _;

pub const ORIGIN_DOMAIN: &str = "z-library.sk";
pub const SNI_DOMAIN: &str = "eliorfoy";

type HttpClient = hyper_util::client::legacy::Client<
    SniConnector,
    http_body_util::combinators::BoxBody<bytes::Bytes, hyper::Error>,
>;

pub static CLIENT: LazyLock<SniBypassClient> = LazyLock::new(SniBypassClient::new);

pub struct SniBypassClient {
    inner: HttpClient,
}

#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: http::HeaderMap,
    body: Option<bytes::Bytes>,
    url: String,
}

impl Response {
    pub fn status(&self) -> http::StatusCode {
        http::StatusCode::from_u16(self.status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR)
    }

    pub fn headers(&self) -> &http::HeaderMap { &self.headers }
    pub fn url(&self) -> &str { &self.url }

    pub async fn text(&mut self) -> Result<String, String> {
        let bytes = self.take_body().await?;
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("UTF-8 解码失败: {e}"))
    }

    pub async fn bytes(&mut self) -> Result<bytes::Bytes, String> {
        self.take_body().await
    }

    async fn take_body(&mut self) -> Result<bytes::Bytes, String> {
        self.body.take().ok_or_else(|| "响应体已被消费".to_string())
    }
}

pub struct RequestBuilder {
    client: HttpClient,
    method: http::Method,
    url: String,
    headers: http::HeaderMap,
    body: Option<bytes::Bytes>,
    max_redirect: u32,
}

impl RequestBuilder {
    pub fn header(mut self, key: http::header::HeaderName, value: &str) -> Self {
        if let Ok(v) = http::HeaderValue::from_str(value) {
            self.headers.insert(key, v);
        }
        self
    }

    pub fn header_str(mut self, name: &str, value: &str) -> Self {
        if let (Ok(k), Ok(v)) = (
            http::header::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_str(value),
        ) {
            self.headers.insert(k, v);
        }
        self
    }

    pub fn body(mut self, body: impl Into<bytes::Bytes>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn form<T: serde::Serialize + ?Sized>(mut self, form: &T) -> Self {
        let encoded = serde_urlencoded::to_string(form).unwrap_or_default();
        self.body = Some(bytes::Bytes::from(encoded));
        self.headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
        );
        self
    }

    pub fn max_redirect(mut self, n: u32) -> Self {
        self.max_redirect = n;
        self
    }

    pub async fn send(self) -> Result<Response, String> {
        send_with_auto_retry(
            self.client,
            self.url,
            self.method,
            self.headers,
            self.body,
            self.max_redirect,
        )
        .await
    }
}

/// 自动处理 502 / 503 重试的入口
///
/// - 502：IP 失效 → `refresh_ip()` 强制换 IP 后重试（最多 1 次）
/// - 503：DiamWall PoW 挑战 → 解析并求解 → 更新全局 cookie 后重试（最多 1 次）
/// - 重试后仍失败则直接返回该响应
async fn send_with_auto_retry(
    client: HttpClient,
    url: String,
    method: http::Method,
    mut headers: http::HeaderMap,
    body: Option<bytes::Bytes>,
    max_redirect: u32,
) -> Result<Response, String> {
    let mut retried_ip = false;
    let mut retried_challenge = 0;

    loop {
        let resp = send_with_redirect(
            client.clone(),
            url.clone(),
            method.clone(),
            headers.clone(),
            body.clone(),
            max_redirect,
        )
        .await?;

        match resp.status().as_u16() {
            502 if !retried_ip => {
                crate::client::ip::refresh_ip().await?;
                retried_ip = true;
                continue;
            }
            503 => {
                let html = String::from_utf8_lossy(
                    &resp.body.clone().unwrap_or_default(),
                )
                .to_string();

                let Some(ch) = crate::client::challenge::parse_challenge(&html)
                else {
                    return Ok(resp);
                };

                // 首次求解：生成新 cookie 并写全局
                if retried_challenge == 0 {
                    let started = Instant::now();
                    let solution = crate::client::challenge::solve(&ch);
                    let elapsed_ms = started.elapsed().as_millis() as u64;

                    let cookie = crate::client::challenge::build_challenge_cookie(
                        &ch, solution, elapsed_ms, None,
                    );

                    // 把新挑战 cookie 写入全局存储，供后续请求复用
                    let pairs: Vec<(String, String)> = cookie
                        .split("; ")
                        .filter_map(|p| p.split_once('='))
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    //crate::account::cookie::update_challenge(&pairs);
                    CHALLENGE_COOKIES.write().unwrap().update_challenge(&pairs);
                }

                // 挑战重试：重写 Cookie 头。
                // 剔除旧挑战段（c_token / c_time / bsrv），保留其余（账号凭据等），
                // 再追加最新挑战 cookie —— 避免新旧两个 c_token 并存被服务端拒绝。
                if retried_challenge == 0 {
                    let old = headers
                        .get(http::header::COOKIE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default();
                    let kept: Vec<&str> = old
                        .split("; ")
                        .filter(|p| {
                            let k = p.split('=').next().unwrap_or("");
                            k != "c_token" && k != "c_time" && k != "bsrv"
                        })
                        .collect();
                    let fresh = crate::client::challenge::CHALLENGE_COOKIES
                        .read()
                        .unwrap()
                        .cookie_str();
                    let mut merged = kept.join("; ");
                    if !merged.is_empty() {
                        merged.push_str("; ");
                    }
                    merged.push_str(&fresh);
                    if let Ok(v) = http::HeaderValue::from_str(&merged) {
                        headers.insert(http::header::COOKIE, v);
                    }
                }

                // 服务端可能按连接限流：短暂等待，让连接池空闲连接失效后新建
                retried_challenge += 1;
                if retried_challenge > 3 {
                    return Ok(resp);
                }
                continue;
            }
            _ => return Ok(resp),
        }
    }
}

/// 构建请求 + 发送 + 自动跟随重定向
async fn send_with_redirect(
    client: HttpClient,
    url: String,
    method: http::Method,
    headers: http::HeaderMap,
    body: Option<bytes::Bytes>,
    remaining: u32,
) -> Result<Response, String> {
    let mut url = url;
    let mut method = method;
    let mut headers = headers;
    let mut body = body;
    let mut remaining = remaining;

    loop {
        let orig_url = url.clone();
        let headers_for_redirect = headers.clone();
        let body_for_redirect = body.clone();

        // ── 构建请求 ─────────────────────────────────────────────
        let mut req = hyper::Request::builder()
            .method(&method)
            .uri(&url);

        #[allow(clippy::collapsible_if)]
        if !headers.contains_key(http::header::HOST) {
            if let Ok(parsed) = url::Url::parse(&url) {
                if let Some(host) = parsed.host_str() {
                    req = req.header(http::header::HOST, host);
                }
            }
        }

        let b = body.unwrap_or_default();
        let req = req
            .body(http_body_util::combinators::BoxBody::new(
                http_body_util::Full::new(b).map_err(|never| match never {}),
            ))
            .map_err(|e| format!("构建请求失败: {e}"))?;

        let (mut parts, b) = req.into_parts();
        parts.headers.extend(headers);
        let req = hyper::Request::from_parts(parts, b);

        // ── 发送请求 ─────────────────────────────────────────────
        let resp = client.request(req).await.map_err(|e| {
            let mut msg = format!("请求失败: {e}");
            let mut source: &dyn std::error::Error = &e;
            while let Some(cause) = source.source() {
                msg.push_str(&format!("\n  原因: {cause}"));
                source = cause;
            }
            msg
        })?;

        let status_code = resp.status().as_u16();
        let rh = resp.headers().clone();

        // ── 消费响应体 ───────────────────────────────────────────
        let (_, body_stream) = resp.into_parts();
        let collected = http_body_util::BodyExt::collect(body_stream)
            .await
            .map_err(|e| format!("读取响应失败: {e}"))?;
        let body_bytes = collected.to_bytes();

        if remaining > 0
            && let Some(location) = rh
                .get(http::header::LOCATION)
                .and_then(|v| v.to_str().ok())
        {
                let mut redirect_headers = headers_for_redirect;
                let cookies: Vec<String> = rh
                    .get_all(http::header::SET_COOKIE)
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .map(|s| s.split(';').next().unwrap_or_default().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !cookies.is_empty() {
                    let mut merged = redirect_headers
                        .get(http::header::COOKIE)
                        .and_then(|v| v.to_str().ok())
                        .map(|s| format!("{s}; "))
                        .unwrap_or_default();
                    merged.push_str(&cookies.join("; "));
                    if let Ok(v) = http::HeaderValue::from_str(&merged) {
                        redirect_headers.insert(http::header::COOKIE, v);
                    }
                }
                url = resolve_url(&orig_url, location);
                method = if matches!(status_code, 301..=303) {
                    http::Method::GET
                } else {
                    method
                };
                body = if method == http::Method::GET {
                    None
                } else {
                    body_for_redirect
                };
                headers = redirect_headers;
                remaining -= 1;
                continue;
            }

        return Ok(Response {
            status: status_code,
            headers: rh,
            body: Some(body_bytes),
            url: orig_url,
        });
    }
}

fn resolve_url(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        location.to_string()
    } else if let Ok(parsed) = url::Url::parse(base).and_then(|u| u.join(location)) {
        parsed.to_string()
    } else {
        let base = base.trim_end_matches('/');
        let loc = location.trim_start_matches('/');
        format!("{base}/{loc}")
    }
}

impl SniBypassClient {
    fn new() -> Self {
        let connector = SniConnector::new();
        let inner = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .pool_max_idle_per_host(4)
            .build(connector);
        Self { inner }
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        RequestBuilder {
            client: self.inner.clone(),
            method: http::Method::GET,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            body: None,
            max_redirect: 10,
        }
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        RequestBuilder {
            client: self.inner.clone(),
            method: http::Method::POST,
            url: url.to_string(),
            headers: http::HeaderMap::new(),
            body: None,
            max_redirect: 10,
        }
    }
}

// ===========================================================================
// TLS 连接器（SNI 伪造 + 跳过证书验证）
// ===========================================================================

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
struct SniConnector {
    /// 伪造 SNI + 跳过证书验证（origin 与 CDN 均走此链路）
    tls_forged: tokio_rustls::TlsConnector,
}

impl SniConnector {
    fn new() -> Self {
        let tls_forged = tokio_rustls::TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifier))
                .with_no_client_auth(),
        ));
        Self { tls_forged }
    }
}

struct SniConnection(tokio_rustls::client::TlsStream<tokio::net::TcpStream>);

impl hyper::rt::Write for SniConnection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        use tokio::io::AsyncWrite;
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        use tokio::io::AsyncWrite;
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        use tokio::io::AsyncWrite;
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl hyper::rt::Read for SniConnection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        use tokio::io::AsyncRead;
        let cap = buf.remaining();
        if cap == 0 { return std::task::Poll::Ready(Ok(())); }
        let mut tmp = vec![0u8; cap];
        let mut read_buf = tokio::io::ReadBuf::new(&mut tmp);
        match std::pin::Pin::new(&mut self.0).poll_read(cx, &mut read_buf)? {
            std::task::Poll::Ready(()) => {
                let n = read_buf.filled().len();
                if n > 0 { buf.put_slice(read_buf.filled()); }
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl hyper_util::client::legacy::connect::Connection for SniConnection {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        hyper_util::client::legacy::connect::Connected::new()
    }
}

impl tower::Service<http::Uri> for SniConnector {
    type Response = SniConnection;
    type Error = BoxError;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: http::Uri) -> Self::Future {
        let tls = self.tls_forged.clone();
        Box::pin(async move {
            let host = uri.host().ok_or("URI 缺少 host")?;
            let port = uri.port_u16().unwrap_or(443);

            let ip = if host == ORIGIN_DOMAIN {
                crate::client::ip::get_ip().await?
            } else {
                format!("{host}:{port}")
                    .to_socket_addrs()
                    .map_err(|e| format!("DNS 解析失败: {e}"))?
                    .find_map(|a| match a.ip() {
                        IpAddr::V4(v4) => Some(v4),
                        _ => None,
                    })
                    .ok_or(format!("无法解析 {host}"))?
            };

            let addr = SocketAddr::new(IpAddr::V4(ip), port);
            let tcp = tokio::net::TcpStream::connect(addr)
                .await
                .map_err(|e| format!("TCP 连接失败 ({addr}): {e}"))?;
            tcp.set_nodelay(true).ok();

            let server_name = rustls::pki_types::ServerName::try_from(SNI_DOMAIN)
                .map_err(|e| format!("无效 SNI '{SNI_DOMAIN}': {e}"))?;
            let tls_stream = tls
                .connect(server_name, tcp)
                .await
                .map_err(|e| format!("TLS 握手失败 (SNI={SNI_DOMAIN}): {e}"))?;

            Ok(SniConnection(tls_stream))
        })
    }
}

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}