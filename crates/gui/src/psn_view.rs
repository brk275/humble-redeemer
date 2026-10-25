//! A user-driven browser, with no injected scripts, account probes, or checkout calls.
use anyhow::{Result, anyhow};
use eframe::egui;
use humble_redeemer_core::{
    psn::{OpenRequest, validate_cookies, validate_navigation},
    store::{data_root, private_dir},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use wry::{
    Rect, WebContext, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};
pub struct ManualView {
    view: Option<WebView>,
    _context: WebContext,
    directory: PathBuf,
    pub request: OpenRequest,
    loaded: Arc<AtomicBool>,
}
impl ManualView {
    pub fn new(
        frame: &eframe::Frame,
        request: OpenRequest,
        cookies: Vec<Value>,
        demo: bool,
        fixture: bool,
    ) -> Result<Self> {
        let directory = data_root(demo)?
            .join("psn-private-view")
            .join(uuid_string());
        private_dir(&directory)?;
        let mut context = WebContext::new(Some(directory.clone()));
        let loaded = Arc::new(AtomicBool::new(false));
        let marker = loaded.clone();
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_incognito(true)
            .with_devtools(false)
            .with_clipboard(true)
            .with_accept_first_mouse(true)
            .with_focused(true)
            .with_url("about:blank")
            .with_on_page_load_handler(move |event, url| {
                if matches!(event, wry::PageLoadEvent::Finished) && url != "about:blank" {
                    marker.store(true, Ordering::Relaxed);
                }
            })
            .with_navigation_handler(move |url| {
                url == "about:blank"
                    || (fixture && fixture_origin(&url))
                    || validate_navigation(&url)
            })
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);
        #[cfg(windows)]
        {
            use wry::WebViewBuilderExtWindows;
            builder = builder.with_additional_browser_args("");
        }
        if fixture {
            // Builder navigation applies Wry's custom-protocol URL mapping on
            // WebView2. A later load_url call does not perform that mapping.
            builder=builder.with_custom_protocol("hrpsnfixture".into(),|_,_|wry::http::Response::builder().header("Content-Type","text/html").body(std::borrow::Cow::Borrowed(b"<!doctype html><html><body style='font:20px system-ui;padding:24px'><h1>Synthetic PlayStation claim page</h1><p>No account or purchase is involved.</p><label>Fixture input <input aria-label='Fixture input'></label><button onclick=\"document.getElementById('result').textContent='Synthetic success: the app still needs a human confirmation.'\">Simulate Add to Library</button><p id='result'></p></body></html>".as_slice())).expect("static fixture")).with_url("hrpsnfixture://localhost/claim");
        }
        let view = builder.build_as_child(frame).map_err(|_| {
            anyhow!("The embedded PlayStation page could not open. Use Open in normal browser.")
        })?;
        validate_cookies(&cookies)?;
        for c in cookies {
            if c["expiry"]
                .as_i64()
                .is_some_and(|t| t <= humble_redeemer_core::model::now())
            {
                continue;
            }
            let (Some(name), Some(value), Some(domain)) = (
                c["name"].as_str(),
                c["value"].as_str(),
                c["domain"].as_str(),
            ) else {
                continue;
            };
            let mut cookie = wry::cookie::Cookie::build((name.to_owned(), value.to_owned()))
                .domain(domain.to_owned())
                .path(c["path"].as_str().unwrap_or("/").to_owned())
                .secure(c["secure"].as_bool().unwrap_or(true))
                .http_only(c["httpOnly"].as_bool().unwrap_or(true));
            if let Some(expiry) = c["expiry"]
                .as_i64()
                .and_then(|t| wry::cookie::time::OffsetDateTime::from_unix_timestamp(t).ok())
            {
                cookie = cookie.expires(expiry);
            }
            view.set_cookie(&cookie.build())?;
        }
        if !fixture {
            view.load_url(&request.url)?;
        }
        Ok(Self {
            view: Some(view),
            _context: context,
            directory,
            request,
            loaded,
        })
    }
    pub fn cookies(&self) -> Result<Vec<Value>> {
        let cookies=self.view.as_ref().ok_or_else(||anyhow!("Browser closed"))?.cookies()?.into_iter().filter(|c|c.domain().is_some_and(|d|humble_redeemer_core::psn::sony_host(d.trim_start_matches('.')))).map(|c|json!({"name":c.name(),"value":c.value(),"domain":c.domain(),"path":c.path().unwrap_or("/"),"secure":c.secure().unwrap_or(true),"httpOnly":c.http_only().unwrap_or(true),"expiry":c.expires_datetime().map(|d|d.unix_timestamp())})).collect();
        Ok(cookies)
    }
    pub fn loaded(&self) -> bool {
        self.loaded.load(Ordering::Relaxed)
    }
    pub fn layout(&self, rect: egui::Rect, visible: bool) {
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
}
fn fixture_origin(url: &str) -> bool {
    url == "hrpsnfixture://localhost/claim" || url == "http://hrpsnfixture.localhost/claim"
}
fn uuid_string() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
impl Drop for ManualView {
    fn drop(&mut self) {
        self.view.take();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
