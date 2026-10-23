//! Windowless HTTP sessions handed off by the app's private native sign-in view.
//! Passwords never pass through this module; only scoped service cookies do.
use crate::{browser::WebResponse, model::Provider};
use anyhow::{Result, anyhow, bail};
use reqwest::{
    cookie::{CookieStore, Jar},
    header,
};
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
}
pub(crate) struct NativeSession {
    provider: Provider,
    client: reqwest::Client,
    jar: Arc<Jar>,
    originals: Mutex<Vec<Value>>,
    pub user_agent: String,
}
impl NativeSession {
    pub fn new(provider: Provider, cookies: Vec<Value>, user_agent: String) -> Result<Self> {
        let jar = Arc::new(Jar::default());
        let origin = reqwest::Url::parse(provider.origin())?;
        let host = origin.host_str().unwrap_or_default();
        let mut accepted = Vec::new();
        for cookie in cookies {
            let Some(name) = cookie["name"].as_str() else {
                continue;
            };
            let Some(value) = cookie["value"].as_str() else {
                continue;
            };
            let domain = cookie["domain"]
                .as_str()
                .unwrap_or(host)
                .trim_start_matches('.');
            if host != domain && !host.ends_with(&format!(".{domain}")) {
                continue;
            }
            if name.contains(['\r', '\n', ';', '=']) || value.contains(['\r', '\n', ';']) {
                continue;
            }
            let path = cookie["path"]
                .as_str()
                .filter(|p| p.starts_with('/') && !p.contains(['\r', '\n', ';']))
                .unwrap_or("/");
            if cookie["expiry"]
                .as_i64()
                .is_some_and(|t| t > 0 && t <= crate::model::now())
            {
                continue;
            }
            let mut encoded = format!("{name}={value}; Domain={domain}; Path={path}");
            if cookie["secure"] == true {
                encoded.push_str("; Secure");
            }
            if cookie["httpOnly"] == true {
                encoded.push_str("; HttpOnly");
            }
            jar.add_cookie_str(&encoded, &origin);
            accepted.push(cookie);
        }
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
            originals: Mutex::new(accepted),
            user_agent,
        })
    }
    pub fn cookies(&self) -> Result<Vec<Value>> {
        let origin = reqwest::Url::parse(self.provider.origin())?;
        let current = self
            .jar
            .cookies(&origin)
            .and_then(|h| h.to_str().ok().map(str::to_owned))
            .unwrap_or_default();
        let originals = self
            .originals
            .lock()
            .map_err(|_| anyhow!("Session unavailable"))?;
        Ok(current.split("; ").filter_map(|pair| {
            let (name,value)=pair.split_once('=')?;
            let mut cookie=originals.iter().find(|c|c["name"]==name).cloned().unwrap_or_else(||json!({"domain":origin.host_str(),"path":"/","secure":true,"httpOnly":true}));
            cookie["name"]=json!(name);cookie["value"]=json!(value);Some(cookie)
        }).collect())
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
        {
            let mut originals = self
                .originals
                .lock()
                .map_err(|_| anyhow!("Session unavailable"))?;
            for cookie in response.cookies() {
                let domain = cookie
                    .domain()
                    .unwrap_or_else(|| response.url().host_str().unwrap_or_default());
                let path = cookie.path().unwrap_or("/");
                originals.retain(|c| {
                    !(c["name"] == cookie.name()
                        && c["domain"]
                            .as_str()
                            .unwrap_or_default()
                            .trim_start_matches('.')
                            == domain.trim_start_matches('.')
                        && c["path"].as_str().unwrap_or("/") == path)
                });
                originals.push(json!({"name":cookie.name(),"value":cookie.value(),"domain":domain,"path":path,"secure":cookie.secure(),"httpOnly":cookie.http_only(),"expiry":cookie.expires().and_then(|t|t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d|d.as_secs())}));
            }
        }
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
            redirected: redirects > 0 || (300..400).contains(&status),
            retry_after,
        })
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
