//! Only official service sign-in lives in a native child WebView. The rest of the UI is egui.
use anyhow::{Result, anyhow};
use humble_redeemer_core::{
    model::{Command, Provider},
    privacy::safe_text,
    store::{data_root, private_dir},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wry::{
    Rect, WebContext, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

pub struct Login {
    view: Option<WebView>,
    _context: WebContext,
    directory: PathBuf,
    events: mpsc::Receiver<Value>,
    pub provider: Provider,
    last_check: Instant,
    pub submitted: bool,
    pub message: String,
    fixture: bool,
    pub fixture_ready: bool,
}
impl Login {
    pub fn new(
        frame: &eframe::Frame,
        provider: Provider,
        demo: bool,
        fixture: bool,
    ) -> Result<Self> {
        let directory = data_root(demo)?.join("native-login").join(format!(
            "{}-{}",
            provider.name(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        private_dir(&directory)?;
        let mut context = WebContext::new(Some(directory.clone()));
        let (tx, events) = mpsc::channel();
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_devtools(false)
            .with_clipboard(true)
            .with_incognito(true)
            .with_accept_first_mouse(true)
            .with_focused(true)
            .with_bounds(Rect {
                position: LogicalPosition::new(220., 200.).into(),
                size: LogicalSize::new(900., 500.).into(),
            })
            .with_navigation_handler(move |url| allowed_navigation(provider, &url))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_ipc_handler(move |request| {
                // No credentials or cookies are sent through JavaScript IPC. Cookie
                // extraction uses the native API only after an authentication check.
                if let Ok(value) = serde_json::from_str::<Value>(request.body()) {
                    let origin = request.uri().to_string();
                    if fixture || allowed_origin(provider, &origin) {
                        let _ = tx.send(value);
                    }
                }
            });
        #[cfg(windows)]
        let builder = {
            use wry::WebViewBuilderExtWindows;
            builder.with_additional_browser_args("")
        };
        let builder = if fixture {
            builder.with_html("<!doctype html><html><body style='font:20px system-ui;padding:30px'><h1>Synthetic sign-in view</h1><label>Fixture text input <input id='fixture-input' autofocus aria-label='Fixture text input'></label><p>No account or network request is made.</p><script>window.fetch=async()=>({status:200,redirected:false,json:async()=>[]});window.ipc.postMessage(JSON.stringify({fixture_loaded:true}));</script></body></html>")
        } else {
            builder.with_url(provider.login())
        };
        let view=builder.build_as_child(frame).map_err(|e|anyhow!("Native sign-in could not open: {}. On Windows install Microsoft's WebView2 Runtime; on Linux use an X11/XWayland session with WebKitGTK.",safe_text(&e.to_string())))?;
        Ok(Self{view:Some(view),_context:context,directory,events,provider,last_check:Instant::now()-Duration::from_secs(10),submitted:false,message:"Enter your credentials directly on the official page below. The app never collects passwords.".into(),fixture,fixture_ready:false})
    }
    pub fn layout(&self, rect: eframe::egui::Rect, visible: bool) {
        if let Some(view) = &self.view {
            let _ = view.set_visible(visible);
            if visible {
                let _ = view.set_bounds(Rect {
                    position: LogicalPosition::new(rect.min.x as f64, rect.min.y as f64).into(),
                    size: LogicalSize::new(rect.width() as f64, rect.height() as f64).into(),
                });
            }
        }
    }
    pub fn poll(&mut self, can_submit: bool) -> Option<Command> {
        let view = self.view.as_ref()?;
        if !self.submitted && self.last_check.elapsed() > Duration::from_secs(4) {
            self.last_check = Instant::now();
            let script = authentication_script(self.provider, self.fixture);
            if view.evaluate_script(&script).is_err() {
                self.message="The sign-in view could not run its session check. Reload or reconnect this account.".into();
            }
        }
        while let Ok(event) = self.events.try_recv() {
            if event["fixture_loaded"] == true {
                self.message = "Synthetic form loaded; checking the authentication bridge…".into();
            }
            if self.fixture && event["authenticated"] == true {
                self.fixture_ready = true;
                self.message =
                    "Synthetic authentication bridge, typing and paste are ready.".into();
                continue;
            }
            if event["check_error"] == true {
                self.message="The provider's session check failed. Finish any security step and verify again.".into();
            }
            if event["wrong_origin"] == true {
                self.message =
                    "Return to the official account page before verifying sign-in.".into();
            }
            if let Some(status) = event["status"].as_u64()
                && status != 200
                && status != 403
            {
                self.message = format!(
                    "Sign-in is not verified yet (HTTP {status}). Finish signing in and verify again."
                );
            }
            if event["status"] == 403 {
                self.message="The provider is showing a security check. Complete it manually on this page; the app cannot bypass it.".into();
            }
            if event["authenticated"] == true && !self.submitted {
                if !can_submit {
                    self.message = "Sign-in detected. Waiting for the local worker to finish its current request…".into();
                    continue;
                }
                let Ok(url) = view.url() else { continue };
                if !allowed_origin(self.provider, &url) {
                    continue;
                }
                let Ok(cookies) = view.cookies() else {
                    self.message =
                        "The native browser could not hand off its session. Try again.".into();
                    continue;
                };
                let cookies=cookies.into_iter().filter(|c|cookie_domain_matches(self.provider,c.domain())).map(|c|json!({"name":c.name(),"value":c.value(),"domain":c.domain(),"path":c.path().unwrap_or("/"),"secure":c.secure().unwrap_or(true),"httpOnly":c.http_only().unwrap_or(false),"expiry":c.expires_datetime().map(|d|d.unix_timestamp())})).collect();
                self.submitted = true;
                self.message = "Verifying the windowless session…".into();
                return Some(Command::AttachNativeSession {
                    provider: self.provider,
                    cookies,
                    user_agent: event["user_agent"]
                        .as_str()
                        .unwrap_or("HumbleRedeemer/0.1.0")
                        .into(),
                });
            }
        }
        None
    }
    pub fn fixture_mode(&self) -> bool {
        self.fixture
    }
    pub fn retry(&mut self) {
        self.submitted = false;
        self.last_check = Instant::now() - Duration::from_secs(10);
    }
}
impl Drop for Login {
    fn drop(&mut self) {
        self.view.take();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn cookie_domain_matches(provider: Provider, domain: Option<&str>) -> bool {
    let Ok(uri) = wry::http::Uri::try_from(provider.origin()) else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    let domain = domain.unwrap_or(host).trim_start_matches('.');
    host == domain || host.ends_with(&format!(".{domain}"))
}
fn authentication_script(provider: Provider, fixture: bool) -> String {
    let path = if provider == Provider::Humble {
        "/api/v1/user/order"
    } else {
        "/account/registerkey"
    };
    let guard = if fixture {
        "false".into()
    } else {
        format!("location.origin!=={}", json!(provider.origin()))
    };
    let body_check = if provider == Provider::Humble {
        "Array.isArray(await r.json().catch(()=>null))"
    } else {
        r#"/\bg_steamID\s*=\s*["'][0-9]{17}["']/.test(await r.text())"#
    };
    let redirect_check = if provider == Provider::Humble {
        "!r.redirected&&"
    } else {
        ""
    };
    format!(
        r#"(()=>{{if({guard}){{window.ipc.postMessage(JSON.stringify({{wrong_origin:true}}));return;}}fetch({path},{{credentials:'same-origin'}}).then(async r=>{{const ok=r.status===200&&{redirect_check}{body_check};window.ipc.postMessage(JSON.stringify({{authenticated:ok,status:r.status,user_agent:navigator.userAgent}}));}}).catch(()=>window.ipc.postMessage(JSON.stringify({{check_error:true}})));}})();"#,
        path = json!(path)
    )
}
fn allowed_origin(provider: Provider, url: &str) -> bool {
    wry::http::Uri::try_from(url).ok().is_some_and(|uri| {
        uri.scheme_str() == Some("https")
            && uri
                .authority()
                .is_some_and(|a| format!("https://{a}") == provider.origin())
    })
}
fn allowed_navigation(provider: Provider, url: &str) -> bool {
    if url == "about:blank" {
        return true;
    }
    let Ok(uri) = wry::http::Uri::try_from(url) else {
        return false;
    };
    if uri.scheme_str() != Some("https") {
        return false;
    }
    let Some(host) = uri.host() else {
        return false;
    };
    match provider {
        Provider::Humble => host == "humblebundle.com" || host.ends_with(".humblebundle.com"),
        Provider::Steam => {
            host == "steampowered.com"
                || host.ends_with(".steampowered.com")
                || host == "steamcommunity.com"
                || host.ends_with(".steamcommunity.com")
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_domain_cookies_are_included_without_leaking_other_sites() {
        assert!(cookie_domain_matches(
            Provider::Humble,
            Some(".humblebundle.com")
        ));
        assert!(cookie_domain_matches(
            Provider::Humble,
            Some("www.humblebundle.com")
        ));
        assert!(!cookie_domain_matches(
            Provider::Humble,
            Some("evilhumblebundle.com")
        ));
        assert!(!cookie_domain_matches(
            Provider::Humble,
            Some("example.invalid")
        ));
        assert!(cookie_domain_matches(
            Provider::Steam,
            Some(".steampowered.com")
        ));
    }
    #[test]
    fn login_navigation_is_service_scoped() {
        assert!(allowed_navigation(
            Provider::Humble,
            "https://www.humblebundle.com/login"
        ));
        assert!(!allowed_navigation(
            Provider::Humble,
            "https://humblebundle.com.example.invalid/login"
        ));
        assert!(!allowed_navigation(
            Provider::Steam,
            "http://store.steampowered.com"
        ));
        assert!(allowed_navigation(
            Provider::Steam,
            "https://login.steampowered.com"
        ));
        assert!(!allowed_origin(Provider::Steam, "https://example.invalid"));
    }
}
