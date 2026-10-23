//! Windowless HTTP sessions handed off by the app's private native sign-in view.
//! Passwords never pass through this module; only scoped service cookies do.
use crate::{browser::WebResponse, model::Provider};
use anyhow::{Result, anyhow, bail};
use reqwest::{cookie::CookieStore, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

#[derive(Serialize, Deserialize)]
pub(crate) struct SavedSession {
    pub cookies: Vec<Value>,
    pub user_agent: String,
    #[serde(default)]
    pub steam_account_id: Option<String>,
}
pub(crate) struct NativeSession {
    provider: Provider,
    client: reqwest::Client,
    jar: Arc<SessionJar>,
    pub user_agent: String,
    pub account_id: Option<String>,
    auth_retry_at: Mutex<i64>,
}
impl NativeSession {
    pub fn new(provider: Provider, cookies: Vec<Value>, user_agent: String) -> Result<Self> {
        let account_id = crate::browser::steam_account_id(&cookies);
        let jar = Arc::new(SessionJar::new(provider, cookies)?);
        let client = reqwest::Client::builder()
            .user_agent(user_agent.clone())
            .cookie_provider(jar.clone())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|_| anyhow!("Native session could not be prepared"))?;
        Ok(Self {
            provider,
            client,
            jar,
            user_agent,
            account_id,
            auth_retry_at: Mutex::new(0),
        })
    }
    pub fn cookies(&self) -> Result<Vec<Value>> {
        self.jar.snapshot()
    }
    pub fn store_cookie_present(&self) -> bool {
        reqwest::Url::parse("https://store.steampowered.com/account/registerkey")
            .ok()
            .and_then(|u| self.jar.cookies(&u))
            .and_then(|h| h.to_str().ok().map(str::to_owned))
            .is_some_and(|s| s.split("; ").any(|c| c.starts_with("steamLoginSecure=")))
    }
    pub fn auth_retry_at(&self) -> i64 {
        self.auth_retry_at.lock().map(|t| *t).unwrap_or(0)
    }
    pub fn defer_auth(&self, delay: Option<u64>) {
        if let Ok(mut t) = self.auth_retry_at.lock() {
            *t =
                crate::model::now().saturating_add(delay.unwrap_or(60).min(i64::MAX as u64) as i64);
        }
    }
    /// Follow Steam's own auth_refresh.js flow. No passwords, browser or key
    /// activation requests are involved. Only the two fixed official hosts run.
    pub async fn renew_steam(&self) -> Result<bool> {
        if self.auth_retry_at() > crate::model::now() {
            bail!(
                "Steam session renewal is waiting for its retry deadline; the saved sign-in was kept"
            );
        }
        if self.provider != Provider::Steam
            || !self
                .cookies()?
                .iter()
                .any(|c| c["name"] == "steamRefresh_steam")
        {
            return Ok(false);
        }
        self.defer_auth(None);
        let response = self
            .client
            .post("https://login.steampowered.com/jwt/ajaxrefresh")
            .header(header::ORIGIN, self.provider.origin())
            .header(header::REFERER, self.provider.origin())
            .form(&[(
                "redir",
                "https://store.steampowered.com/account/registerkey",
            )])
            .send()
            .await
            .map_err(|_| {
                anyhow!("Steam session renewal interrupted; the saved sign-in was kept")
            })?;
        let response = read_response(response, false).await?;
        if response.status == 429 {
            self.defer_auth(Some(response.retry_after.unwrap_or(3600)));
        }
        check_auth_status(&response)?;
        if response.body["success"] != true {
            return Ok(false);
        }
        let url = refresh_target(&response.body)?;
        let mut fields = response
            .body
            .as_object()
            .ok_or_else(|| anyhow!("Steam returned an incomplete renewal response"))?
            .iter()
            .filter(|(k, _)| k.as_str() != "login_url")
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string()),
                )
            })
            .collect::<Vec<_>>();
        if let Some(prior) = self
            .cookies()?
            .iter()
            .find(|c| c["name"] == "steamLoginSecure")
            .and_then(|c| c["value"].as_str())
            .and_then(|v| urlencoding::decode(v).ok())
            .and_then(|v| v.split_once("||").map(|(_, token)| token.to_owned()))
        {
            fields.push(("prior".into(), prior));
        }
        let response = self
            .client
            .post(url)
            .header(header::ORIGIN, self.provider.origin())
            .header(header::REFERER, self.provider.origin())
            .form(&fields)
            .send()
            .await
            .map_err(|_| {
                anyhow!("Steam session renewal interrupted; the saved sign-in was kept")
            })?;
        let response = read_response(response, false).await?;
        if response.status == 429 {
            self.defer_auth(Some(response.retry_after.unwrap_or(3600)));
        }
        check_auth_status(&response)?;
        Ok(response.status == 200 && response.body["result"] == 1)
    }
    pub async fn fetch(&self, path: &str, payload: Option<Value>) -> Result<WebResponse> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
            bail!("Only service-relative requests are allowed");
        }
        let url = reqwest::Url::parse(self.provider.origin())?.join(path)?;
        if url.origin().ascii_serialization() != self.provider.origin() {
            bail!("Untrusted service origin");
        }
        let is_get = payload.is_none();
        let mut request = if let Some(payload) = payload {
            let fields = payload
                .as_object()
                .ok_or_else(|| anyhow!("Invalid service request"))?
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect::<Vec<_>>();
            let cookies = self.cookies()?;
            let csrf = cookies
                .iter()
                .find(|c| c["name"] == "csrf_cookie")
                .and_then(|c| c["value"].as_str())
                .unwrap_or_default();
            self.client
                .post(url)
                .form(&fields)
                .header(header::ORIGIN, self.provider.origin())
                .header("csrf-prevention-token", csrf)
        } else {
            self.client.get(url)
        };
        request = request.header(header::REFERER, self.provider.origin());
        let mut redirects = 0;
        let response = loop {
            let response = request
                .send()
                .await
                .map_err(|_| anyhow!("Service request interrupted; reconcile before retrying"))?;
            // Read-only provider pages can move within their official origin.
            // Never replay a reveal, claim or activation after a redirect.
            if is_get
                && response.status().is_redirection()
                && let Some(location) = response.headers().get(header::LOCATION)
            {
                // The expired Steam store session can point at its separate
                // login host. Do not follow that redirect or run its page;
                // return the status so the explicit renewal flow can handle it.
                if self.provider == Provider::Steam
                    && path == "/account/registerkey"
                    && location
                        .to_str()
                        .ok()
                        .and_then(|s| response.url().join(s).ok())
                        .is_some_and(|u| {
                            u.origin().ascii_serialization() == "https://login.steampowered.com"
                                && u.path() == "/jwt/refresh"
                                && u.username().is_empty()
                                && u.password().is_none()
                        })
                {
                    break response;
                }
                if redirects >= 5 {
                    bail!("Service page redirected too many times; reconnect and retry");
                }
                let target = redirect_target(response.url(), location, self.provider)?;
                request = self
                    .client
                    .get(target)
                    .header(header::REFERER, self.provider.origin());
                redirects += 1;
                continue;
            }
            break response;
        };
        read_response(response, redirects > 0).await
    }
}
async fn read_response(response: reqwest::Response, redirected: bool) -> Result<WebResponse> {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(retry_delay);
    if response
        .content_length()
        .is_some_and(|s| s > 25 * 1024 * 1024)
    {
        bail!("Service response exceeded its size limit");
    }
    let text = response
        .text()
        .await
        .map_err(|_| anyhow!("Service response interrupted"))?;
    if text.len() > 25 * 1024 * 1024 {
        bail!("Service response exceeded its size limit");
    }
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Ok(WebResponse {
        status,
        body,
        redirected: redirected || (300..400).contains(&status),
        retry_after,
    })
}
fn check_auth_status(response: &WebResponse) -> Result<()> {
    if response.status == 429 {
        bail!("Steam requested a session-check cooldown; the saved sign-in was kept");
    }
    if response.status == 403 || response.status >= 500 || response.redirected {
        bail!("Steam session verification is temporarily unavailable; the saved sign-in was kept");
    }
    if response.status != 200 && response.status != 401 {
        bail!("Steam returned an incomplete session response; the saved sign-in was kept");
    }
    Ok(())
}
fn refresh_target(body: &Value) -> Result<reqwest::Url> {
    let url = body["login_url"]
        .as_str()
        .and_then(|s| reqwest::Url::parse(s).ok())
        .ok_or_else(|| anyhow!("Steam returned an incomplete renewal response"))?;
    if url.origin().ascii_serialization() != Provider::Steam.origin()
        || url.path() != "/login/settoken"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Steam returned an unexpected renewal destination; the saved sign-in was kept");
    }
    Ok(url)
}
/// Keep every scoped cookie, including login-host and non-root-path cookies.
/// reqwest's header-only Jar snapshot loses these and reuses stale expiry data.
struct SessionJar {
    provider: Provider,
    store: Mutex<cookie_store::CookieStore>,
}
impl SessionJar {
    fn new(provider: Provider, cookies: Vec<Value>) -> Result<Self> {
        let mut store = cookie_store::CookieStore::default();
        let origin = reqwest::Url::parse(provider.origin())?;
        for cookie in cookies {
            let Some(name) = cookie["name"].as_str() else {
                continue;
            };
            let Some(value) = cookie["value"].as_str() else {
                continue;
            };
            let domain = cookie["domain"]
                .as_str()
                .unwrap_or(origin.host_str().unwrap_or_default())
                .trim_start_matches('.');
            if !provider.accepts_cookie_domain(domain)
                || name.contains(['\r', '\n', ';', '='])
                || value.contains(['\r', '\n', ';'])
            {
                continue;
            }
            let path = cookie["path"]
                .as_str()
                .filter(|p| p.starts_with('/') && !p.contains(['\r', '\n', ';']))
                .unwrap_or("/");
            let mut encoded = format!("{name}={value}; Path={path}");
            if cookie["hostOnly"] != true {
                encoded.push_str(&format!("; Domain={domain}"));
            }
            if cookie["secure"] != false {
                encoded.push_str("; Secure");
            }
            if cookie["httpOnly"] == true {
                encoded.push_str("; HttpOnly");
            }
            if let Some(expiry) = cookie["expiry"].as_i64().filter(|e| *e > 0) {
                if expiry <= crate::model::now() {
                    continue;
                }
                encoded.push_str(&format!(
                    "; Expires={}",
                    httpdate::fmt_http_date(
                        SystemTime::UNIX_EPOCH + Duration::from_secs(expiry as u64)
                    )
                ));
            }
            let url = reqwest::Url::parse(&format!("https://{domain}{path}"))?;
            let _ = store.parse(&encoded, &url);
        }
        Ok(Self {
            provider,
            store: Mutex::new(store),
        })
    }
    fn snapshot(&self) -> Result<Vec<Value>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow!("Session unavailable"))?;
        let mut cookies = store.iter_unexpired().map(|c|json!({
            "name":c.name(),"value":c.value(),"domain":String::from(&c.domain),"path":String::from(&c.path),
            "hostOnly":matches!(c.domain,cookie_store::CookieDomain::HostOnly(_)),
            "secure":c.secure().unwrap_or(false),"httpOnly":c.http_only().unwrap_or(false),
            "expiry":match c.expires {cookie_store::CookieExpiration::AtUtc(t)=>Some(t.unix_timestamp()),_=>None}
        })).collect::<Vec<_>>();
        cookies.sort_by_key(|c| format!("{}:{}:{}", c["domain"], c["path"], c["name"]));
        Ok(cookies)
    }
}
impl CookieStore for SessionJar {
    fn set_cookies(
        &self,
        headers: &mut dyn Iterator<Item = &header::HeaderValue>,
        url: &reqwest::Url,
    ) {
        if url.scheme() != "https"
            || !self
                .provider
                .accepts_cookie_domain(url.host_str().unwrap_or_default())
        {
            return;
        }
        if let Ok(mut store) = self.store.lock() {
            for h in headers {
                if let Ok(text) = h.to_str()
                    && let Ok(cookie) = cookie_store::Cookie::parse(text.to_owned(), url)
                    && self
                        .provider
                        .accepts_cookie_domain(&String::from(&cookie.domain))
                {
                    let _ = store.insert(cookie.into_owned(), url);
                }
            }
        }
    }
    fn cookies(&self, url: &reqwest::Url) -> Option<header::HeaderValue> {
        if url.scheme() != "https"
            || !self
                .provider
                .accepts_cookie_domain(url.host_str().unwrap_or_default())
        {
            return None;
        }
        let store = self.store.lock().ok()?;
        let value = store
            .get_request_values(url)
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            None
        } else {
            header::HeaderValue::from_str(&value).ok()
        }
    }
}
fn redirect_target(
    current: &reqwest::Url,
    location: &header::HeaderValue,
    provider: Provider,
) -> Result<reqwest::Url> {
    let target = location
        .to_str()
        .ok()
        .and_then(|s| current.join(s).ok())
        .ok_or_else(|| anyhow!("Service returned an invalid redirect"))?;
    if target.origin().ascii_serialization() != provider.origin()
        || !target.username().is_empty()
        || target.password().is_some()
    {
        bail!("Service redirected outside its official origin; reconnect in the app");
    }
    Ok(target)
}
fn retry_delay(value: &str) -> Option<u64> {
    value.trim().parse().ok().or_else(|| {
        httpdate::parse_http_date(value).ok().map(|t| {
            t.duration_since(SystemTime::now())
                .unwrap_or_default()
                .as_secs()
        })
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn apply_cookie(jar: &SessionJar, url: &str, value: &str) {
        let header = header::HeaderValue::from_str(value).unwrap();
        jar.set_cookies(
            &mut std::iter::once(&header),
            &reqwest::Url::parse(url).unwrap(),
        );
    }
    #[test]
    fn all_scopes_and_rotated_expiry_survive_restart_and_deletions_do_not() {
        let jar = SessionJar::new(Provider::Steam, vec![]).unwrap();
        apply_cookie(
            &jar,
            "https://login.steampowered.com/jwt/ajaxrefresh",
            "steamRefresh_steam=renewable-fixture; Path=/jwt; Max-Age=7200; Secure; HttpOnly",
        );
        apply_cookie(
            &jar,
            "https://store.steampowered.com/login/settoken",
            "steamLoginSecure=old-fixture; Path=/; Max-Age=1; Secure; HttpOnly",
        );
        apply_cookie(
            &jar,
            "https://store.steampowered.com/login/settoken",
            "steamLoginSecure=new-fixture; Path=/; Max-Age=3600; Secure; HttpOnly",
        );
        apply_cookie(
            &jar,
            "https://store.steampowered.com/",
            "deleted=fixture; Path=/; Secure",
        );
        apply_cookie(
            &jar,
            "https://store.steampowered.com/",
            "deleted=; Path=/; Max-Age=0; Secure",
        );
        apply_cookie(
            &jar,
            "https://store.steampowered.com/",
            "outside=no; Domain=example.invalid; Path=/; Secure",
        );
        let cookies = jar.snapshot().unwrap();
        assert_eq!(cookies.len(), 2);
        let access = cookies
            .iter()
            .find(|c| c["name"] == "steamLoginSecure")
            .unwrap();
        assert_eq!(access["value"], "new-fixture");
        assert!(access["expiry"].as_i64().unwrap() >= crate::model::now() + 3598);
        assert_eq!(access["hostOnly"], true);
        let restored = SessionJar::new(Provider::Steam, cookies.clone()).unwrap();
        assert_eq!(restored.snapshot().unwrap(), cookies);
        assert!(
            restored
                .cookies(
                    &reqwest::Url::parse("https://login.steampowered.com/jwt/ajaxrefresh").unwrap()
                )
                .unwrap()
                .to_str()
                .unwrap()
                .contains("steamRefresh_steam")
        );
        let store_header = restored
            .cookies(&reqwest::Url::parse(Provider::Steam.origin()).unwrap())
            .unwrap();
        assert_eq!(
            store_header.to_str().unwrap(),
            "steamLoginSecure=new-fixture"
        );
        assert!(
            restored
                .cookies(&reqwest::Url::parse("https://example.invalid/").unwrap())
                .is_none()
        );
    }
    #[test]
    fn renewal_transfers_cannot_leave_the_exact_store_endpoint() {
        assert!(
            refresh_target(&json!({"login_url":"https://store.steampowered.com/login/settoken"}))
                .is_ok()
        );
        for url in [
            "https://example.invalid/login/settoken",
            "https://store.steampowered.com.example.invalid/login/settoken",
            "http://store.steampowered.com/login/settoken",
            "https://user@store.steampowered.com/login/settoken",
            "https://store.steampowered.com/account/ajaxregisterkey/",
            "https://store.steampowered.com/login/settoken?redirect=elsewhere",
        ] {
            assert!(refresh_target(&json!({"login_url":url})).is_err());
        }
    }
    #[tokio::test]
    async fn official_renewal_sequence_rotates_cookies_and_restores_without_a_password() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::{
            TlsAcceptor,
            rustls::{self, pki_types::PrivatePkcs8KeyDer},
        };
        let cert = rcgen::generate_simple_self_signed(vec![
            "login.steampowered.com".into(),
            "store.steampowered.com".into(),
        ])
        .unwrap();
        let der = cert.cert.der().clone();
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![der.clone()],
                PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
            )
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let server = tokio::spawn(async move {
            for expected in [
                "/jwt/ajaxrefresh",
                "/login/settoken",
                "/account/registerkey",
            ] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 65536);
                }
                let headers = String::from_utf8(request).unwrap();
                let length = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                assert!(length < 65536);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                assert_eq!(headers.split_whitespace().nth(1), Some(expected));
                let (body, cookies) = match expected {
                    "/jwt/ajaxrefresh" => {
                        assert!(headers.contains("steamRefresh_steam=initial-fixture"));
                        assert!(!headers.contains("steamLoginSecure="));
                        assert!(String::from_utf8(body).unwrap().contains("redir="));
                        (json!({"success":true,"login_url":"https://store.steampowered.com/login/settoken","nonce":"synthetic-nonce"}).to_string(),"Set-Cookie: steamRefresh_steam=rotated-fixture; Path=/jwt; Max-Age=7200; Secure; HttpOnly\r\n")
                    }
                    "/login/settoken" => {
                        assert!(!headers.contains("steamRefresh_steam="));
                        assert!(
                            String::from_utf8(body)
                                .unwrap()
                                .contains("nonce=synthetic-nonce")
                        );
                        (
                            json!({"result":1}).to_string(),
                            "Set-Cookie: steamLoginSecure=12345678901234567%7C%7Caccess-fixture; Path=/; Max-Age=3600; Secure; HttpOnly\r\nSet-Cookie: sessionid=fixture-session; Path=/; Secure\r\n",
                        )
                    }
                    _ => {
                        assert!(headers.contains("steamLoginSecure="));
                        assert!(!headers.contains("steamRefresh_steam="));
                        (
                            "<script>g_steamID = \"12345678901234567\";</script>".into(),
                            "",
                        )
                    }
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{cookies}\r\n{body}",body.len()).as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        let setup = |cookies| {
            let mut session =
                NativeSession::new(Provider::Steam, cookies, "synthetic-test".into()).unwrap();
            session.client = reqwest::Client::builder()
                .no_proxy()
                .cookie_provider(session.jar.clone())
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .add_root_certificate(reqwest::Certificate::from_der(&der).unwrap())
                .resolve("login.steampowered.com", address)
                .resolve("store.steampowered.com", address)
                .build()
                .unwrap();
            session
        };
        let session = setup(vec![
            json!({"name":"steamRefresh_steam","value":"initial-fixture","domain":"login.steampowered.com","path":"/jwt","secure":true}),
        ]);
        assert!(session.renew_steam().await.unwrap());
        let saved = session.cookies().unwrap();
        assert!(saved.iter().any(|c| c["value"] == "rotated-fixture"));
        let restored = setup(saved);
        let page = restored.fetch("/account/registerkey", None).await.unwrap();
        assert_eq!(page.status, 200);
        assert_eq!(
            crate::browser::steam_account_id(&restored.cookies().unwrap()).as_deref(),
            Some("12345678901234567")
        );
        server.await.unwrap();
    }
    #[test]
    fn cookies_are_scoped_and_retry_guidance_is_understood() {
        let session=NativeSession::new(Provider::Steam,vec![json!({"name":"sessionid","value":"fixture","domain":"store.steampowered.com","path":"/","secure":true}),json!({"name":"outside","value":"nope","domain":"example.invalid"})],"synthetic-test".into()).unwrap();
        let cookies = session.cookies().unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0]["name"], "sessionid");
        assert_eq!(retry_delay("3600"), Some(3600));
        assert!(
            retry_delay(&httpdate::fmt_http_date(
                SystemTime::now() + Duration::from_secs(7200)
            ))
            .unwrap()
                >= 7198
        );
    }
    #[test]
    fn read_only_redirects_stay_on_the_official_origin() {
        let current =
            reqwest::Url::parse("https://www.humblebundle.com/subscription/fixture").unwrap();
        let target = redirect_target(
            &current,
            &header::HeaderValue::from_static("/membership/fixture"),
            Provider::Humble,
        )
        .unwrap();
        assert_eq!(target.path(), "/membership/fixture");
        for location in [
            "https://example.invalid/",
            "//example.invalid/",
            "http://www.humblebundle.com/",
            "https://user@www.humblebundle.com/",
        ] {
            assert!(
                redirect_target(
                    &current,
                    &header::HeaderValue::from_str(location).unwrap(),
                    Provider::Humble
                )
                .is_err()
            );
        }
    }
}
