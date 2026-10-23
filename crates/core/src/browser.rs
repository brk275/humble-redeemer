//! Browser sign-in is human-operated. This adapter never accepts passwords or bypasses challenges.
use crate::{model::*, store::private_dir};
use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::Duration,
};

pub fn installed_browser(kind: BrowserKind) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let suffix = match kind {
            BrowserKind::Chrome => "Google Chrome.app/Contents/MacOS/Google Chrome",
            BrowserKind::Firefox => "Firefox.app/Contents/MacOS/firefox",
        };
        candidates.push(PathBuf::from("/Applications").join(suffix));
        if let Some(home) = directories::BaseDirs::new() {
            candidates.push(home.home_dir().join("Applications").join(suffix));
        }
    }
    #[cfg(target_os = "windows")]
    {
        for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Some(path) = std::env::var_os(root) {
                candidates.push(PathBuf::from(path).join(match kind {
                    BrowserKind::Chrome => "Google/Chrome/Application/chrome.exe",
                    BrowserKind::Firefox => "Mozilla Firefox/firefox.exe",
                }));
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for path in match kind {
            BrowserKind::Chrome => vec![
                "/usr/bin/google-chrome",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
            ],
            BrowserKind::Firefox => vec!["/usr/bin/firefox", "/snap/bin/firefox"],
        } {
            candidates.push(PathBuf::from(path));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}
pub fn available_browsers() -> Vec<BrowserKind> {
    [BrowserKind::Firefox, BrowserKind::Chrome]
        .into_iter()
        .filter(|kind| installed_browser(*kind).is_some())
        .collect()
}
fn driver_name(kind: BrowserKind) -> String {
    format!(
        "{}{}",
        match kind {
            BrowserKind::Firefox => "geckodriver",
            BrowserKind::Chrome => "chromedriver",
        },
        std::env::consts::EXE_SUFFIX
    )
}
fn platform(kind: BrowserKind) -> Result<&'static str> {
    match (kind, std::env::consts::OS, std::env::consts::ARCH) {
        (BrowserKind::Chrome, "macos", "aarch64") => Ok("mac-arm64"),
        (BrowserKind::Chrome, "macos", "x86_64") => Ok("mac-x64"),
        (BrowserKind::Chrome, "windows", "x86_64") => Ok("win64"),
        (BrowserKind::Chrome, "linux", "x86_64") => Ok("linux64"),
        (BrowserKind::Firefox, "macos", "aarch64") => Ok("macos-aarch64.tar.gz"),
        (BrowserKind::Firefox, "macos", "x86_64") => Ok("macos.tar.gz"),
        (BrowserKind::Firefox, "windows", "x86_64") => Ok("win64.zip"),
        (BrowserKind::Firefox, "linux", "x86_64") => Ok("linux64.tar.gz"),
        _ => bail!("Driver provisioning is unavailable for this platform"),
    }
}
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "HumbleRedeemer/",
            env!("CARGO_PKG_VERSION"),
            " (unofficial desktop utility)"
        ))
        .timeout(Duration::from_secs(40))
        .build()?)
}

#[derive(Serialize, Deserialize)]
struct DriverReceipt {
    binary_sha256: String,
    archive_sha256: String,
    source: String,
    version: String,
}
#[cfg(not(windows))]
fn chrome_build() -> Result<String> {
    let browser = installed_browser(BrowserKind::Chrome)
        .ok_or_else(|| anyhow!("Install Chrome before provisioning its driver"))?;
    let output = std::process::Command::new(browser)
        .arg("--version")
        .output()
        .map_err(|_| anyhow!("Could not determine Chrome version"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let re = regex::Regex::new(r"\d+\.\d+\.\d+\.\d+")?;
    let version = re
        .find(&text)
        .ok_or_else(|| anyhow!("Chrome version could not be read"))?
        .as_str();
    Ok(version
        .rsplit_once('.')
        .ok_or_else(|| anyhow!("Invalid Chrome version"))?
        .0
        .into())
}
#[cfg(windows)]
fn chrome_build() -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let browser =
        installed_browser(BrowserKind::Chrome).ok_or_else(|| anyhow!("Install Chrome first"))?;
    let path: Vec<u16> = browser.as_os_str().encode_wide().chain(Some(0)).collect();
    // Windows owns neither buffer: sizes come from the version API and all pointers
    // remain valid for the duration of these calls. Copy the potentially unaligned result.
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1024 * 1024 {
            bail!("Chrome version information unavailable");
        }
        let mut buffer = vec![0u8; size as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) == 0 {
            bail!("Chrome version information unavailable");
        }
        let mut pointer = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            buffer.as_ptr().cast(),
            [92u16, 0].as_ptr(),
            &mut pointer,
            &mut length,
        ) == 0
            || pointer.is_null()
            || length < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
        {
            bail!("Chrome version information invalid");
        }
        let info = std::ptr::read_unaligned(pointer.cast::<VS_FIXEDFILEINFO>());
        Ok(format!(
            "{}.{}.{}",
            info.dwFileVersionMS >> 16,
            info.dwFileVersionMS & 0xffff,
            info.dwFileVersionLS >> 16
        ))
    }
}
fn matching_build(driver: &str, browser_build: &str) -> bool {
    driver
        .rsplit_once('.')
        .is_some_and(|(build, _)| build == browser_build)
}
pub async fn provision_driver(root: &Path, kind: BrowserKind) -> Result<PathBuf> {
    let directory = root.join("drivers").join(kind.name());
    private_dir(&directory)?;
    let destination = directory.join(driver_name(kind));
    let receipt_path = directory.join("integrity.json");
    let browser_build = if kind == BrowserKind::Chrome {
        Some(chrome_build()?)
    } else {
        None
    };
    if let (Ok(bytes), Ok(receipt)) = (std::fs::read(&destination), std::fs::read(&receipt_path)) {
        let receipt = serde_json::from_slice::<DriverReceipt>(&receipt)
            .map_err(|_| anyhow!("Cached driver receipt is invalid; remove the private drivers folder and provision again"))?;
        if hex::encode(Sha256::digest(&bytes)) != receipt.binary_sha256 {
            bail!(
                "Cached driver failed integrity verification; remove the private drivers folder and provision again"
            );
        }
        if browser_build
            .as_ref()
            .is_none_or(|build| matching_build(&receipt.version, build))
        {
            return Ok(destination);
        }
        // A browser update needs a newly verified, compatible driver.
    }
    let client = http_client()?;
    let (url, trusted_sha, version) = if kind == BrowserKind::Firefox {
        let meta: Value = client
            .get("https://api.github.com/repos/mozilla/geckodriver/releases/latest")
            .send()
            .await
            .map_err(|_| anyhow!("Mozilla driver metadata unavailable"))?
            .error_for_status()?
            .json()
            .await?;
        let suffix = platform(kind)?;
        let asset = meta["assets"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|a| a["name"].as_str().is_some_and(|s| s.ends_with(suffix)))
            })
            .ok_or_else(|| anyhow!("No Mozilla driver for this platform"))?;
        let digest = asset["digest"].as_str().and_then(|s| s.strip_prefix("sha256:")).ok_or_else(|| anyhow!("Mozilla release lacks a SHA-256 digest; use Chrome or a verified newer driver release"))?;
        (
            asset["browser_download_url"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            Some(digest.to_owned()),
            meta["tag_name"].as_str().unwrap_or_default().to_owned(),
        )
    } else {
        let build = browser_build
            .as_deref()
            .ok_or_else(|| anyhow!("Chrome version unavailable"))?;
        let meta: Value = client.get("https://googlechromelabs.github.io/chrome-for-testing/latest-patch-versions-per-build-with-downloads.json").send().await?.error_for_status()?.json().await?;
        let entry = &meta["builds"][build];
        let asset = entry["downloads"]["chromedriver"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|a| a["platform"] == platform(kind).unwrap_or_default())
            })
            .ok_or_else(|| {
                anyhow!("No matching official ChromeDriver. Update Chrome, then try again.")
            })?;
        (
            asset["url"].as_str().unwrap_or_default().to_owned(),
            None,
            entry["version"].as_str().unwrap_or_default().to_owned(),
        )
    };
    if !(url.starts_with("https://storage.googleapis.com/chrome-for-testing-public/")
        || url.starts_with("https://github.com/mozilla/geckodriver/releases/download/"))
    {
        bail!("Untrusted driver download location");
    }
    let response = client.get(&url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > 100 * 1024 * 1024)
    {
        bail!("Driver archive exceeds size limit");
    }
    let google_hash = response
        .headers()
        .get("x-goog-hash")
        .and_then(|s| s.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await?;
    if bytes.len() > 100 * 1024 * 1024 {
        bail!("Driver archive exceeds size limit");
    }
    let sha = hex::encode(Sha256::digest(&bytes));
    if let Some(expected) = trusted_sha {
        if sha != expected {
            bail!("Mozilla driver SHA-256 mismatch");
        }
    } else {
        use md5::Md5;
        let expected = if let Some(hash) = google_hash
            .as_deref()
            .and_then(|h| h.split(',').find_map(|p| p.trim().strip_prefix("md5=")))
        {
            hash.to_owned()
        } else {
            // Some Google objects expose CRC32C only in headers. Ask the official
            // object-metadata API for its MD5 rather than dropping verification.
            let object = url
                .strip_prefix("https://storage.googleapis.com/chrome-for-testing-public/")
                .ok_or_else(|| anyhow!("Invalid Google driver object"))?;
            let metadata: Value = client
                .get(format!(
                    "https://storage.googleapis.com/storage/v1/b/chrome-for-testing-public/o/{}",
                    urlencoding::encode(object)
                ))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if metadata["name"] != object
                || metadata["size"]
                    .as_str()
                    .and_then(|s| s.parse::<usize>().ok())
                    != Some(bytes.len())
            {
                bail!("Google driver object metadata did not match the download");
            }
            metadata["md5Hash"]
                .as_str()
                .ok_or_else(|| anyhow!("Google did not supply a verifiable archive digest"))?
                .to_owned()
        };
        let actual = base64::engine::general_purpose::STANDARD.encode(Md5::digest(&bytes));
        if expected != actual {
            bail!("Google driver archive integrity mismatch");
        }
    }
    let binary = unpack_driver(&bytes, &driver_name(kind), url.ends_with(".zip"))?;
    let receipt = DriverReceipt {
        binary_sha256: hex::encode(Sha256::digest(&binary)),
        archive_sha256: sha,
        source: url,
        version,
    };
    let temporary = destination.with_extension("download");
    std::fs::write(&temporary, binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::rename(&temporary, &destination)?;
    std::fs::write(receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
    Ok(destination)
}
fn unpack_driver(bytes: &[u8], name: &str, zip: bool) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    if zip {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            if file
                .enclosed_name()
                .and_then(|p| p.file_name().map(|n| n == name))
                .unwrap_or(false)
                && file.is_file()
            {
                if file.size() > 50 * 1024 * 1024 {
                    bail!("Driver binary too large");
                }
                file.read_to_end(&mut output)?;
                break;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(bytes)));
        for entry in archive.entries()? {
            let mut file = entry?;
            if file.path()?.file_name().is_some_and(|n| n == name)
                && file.header().entry_type().is_file()
            {
                if file.size() > 50 * 1024 * 1024 {
                    bail!("Driver binary too large");
                }
                file.read_to_end(&mut output)?;
                break;
            }
        }
    }
    if output.is_empty() {
        bail!("Official archive did not contain its expected driver");
    }
    Ok(output)
}

pub struct Browser {
    child: Option<Child>,
    endpoint: String,
    session: String,
    http: reqwest::Client,
    pub kind: BrowserKind,
    pub provider: Provider,
    profile: PathBuf,
    native: Option<crate::session::NativeSession>,
}
#[derive(Deserialize)]
pub struct WebResponse {
    pub status: u16,
    pub body: Value,
    #[serde(default)]
    pub redirected: bool,
    #[serde(default)]
    pub retry_after: Option<u64>,
}
impl Browser {
    #[cfg(test)]
    pub(crate) fn fixture(endpoint: String, provider: Provider) -> Self {
        Self {
            native: None,
            child: None,
            endpoint,
            session: "fixture".into(),
            http: http_client().unwrap(),
            kind: BrowserKind::Chrome,
            provider,
            profile: PathBuf::new(),
        }
    }
    pub(crate) fn from_native(
        provider: Provider,
        cookies: Vec<Value>,
        user_agent: String,
    ) -> Result<Self> {
        Ok(Self {
            child: None,
            endpoint: String::new(),
            session: String::new(),
            http: http_client()?,
            kind: BrowserKind::Chrome,
            provider,
            profile: PathBuf::new(),
            native: Some(crate::session::NativeSession::new(
                provider, cookies, user_agent,
            )?),
        })
    }
    pub(crate) fn native_snapshot(&self) -> Result<Option<crate::session::SavedSession>> {
        self.native
            .as_ref()
            .map(|native| {
                Ok(crate::session::SavedSession {
                    cookies: native.cookies()?,
                    user_agent: native.user_agent.clone(),
                })
            })
            .transpose()
    }
    pub async fn open(
        root: &Path,
        provider: Provider,
        preferred: Option<BrowserKind>,
        saved: Option<Vec<Value>>,
    ) -> Result<Self> {
        let kind = preferred
            .or_else(|| available_browsers().first().copied())
            .ok_or_else(|| anyhow!("Install Firefox or Chrome, then select Connect again"))?;
        let binary = installed_browser(kind)
            .ok_or_else(|| anyhow!("The selected browser is not installed"))?;
        let driver = provision_driver(root, kind).await?;
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let profile = root
            .join("browser-profiles")
            .join(uuid::Uuid::new_v4().to_string());
        private_dir(&profile)?;
        let mut command = std::process::Command::new(driver);
        match kind {
            BrowserKind::Chrome => {
                command
                    .arg(format!("--port={port}"))
                    .arg("--allowed-ips=127.0.0.1");
            }
            BrowserKind::Firefox => {
                command.args(["--port", &port.to_string(), "--host", "127.0.0.1"]);
            }
        }
        let child = command
            .env("TMPDIR", &profile)
            .env("TMP", &profile)
            .env("TEMP", &profile)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| anyhow!("Browser driver could not start"))?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let http = http_client()?;
        let mut this = Self {
            native: None,
            child: Some(child),
            endpoint,
            session: String::new(),
            http,
            kind,
            provider,
            profile,
        };
        let mut ready = false;
        for _ in 0..50 {
            if this
                .http
                .get(format!("{}/status", this.endpoint))
                .send()
                .await
                .is_ok()
            {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            bail!("Browser driver did not become ready");
        }
        let capabilities = match kind {
            BrowserKind::Chrome => {
                json!({"browserName":"chrome", "goog:chromeOptions":{"binary":binary,"args":[format!("--user-data-dir={}",this.profile.display()),"--no-first-run","--incognito"],"prefs":{"credentials_enable_service":false,"profile.password_manager_enabled":false}}})
            }
            BrowserKind::Firefox => {
                json!({"browserName":"firefox", "moz:firefoxOptions":{"binary":binary,"prefs":{"signon.rememberSignons":false,"browser.privatebrowsing.autostart":true}}})
            }
        };
        let result: Value = this
            .http
            .post(format!("{}/session", this.endpoint))
            .json(&json!({"capabilities":{"alwaysMatch":capabilities}}))
            .send()
            .await
            .map_err(|_| anyhow!("Browser connection failed"))?
            .json()
            .await?;
        this.session = result["value"]["sessionId"]
            .as_str()
            .ok_or_else(|| {
                anyhow!("Browser rejected this driver; update the browser and provision again")
            })?
            .into();
        this.command(
            "POST",
            "timeouts",
            Some(json!({"script":25000,"pageLoad":45000})),
        )
        .await?;
        this.goto(provider.login()).await?;
        if let Some(cookies) = saved {
            for mut cookie in cookies {
                if let Some(object) = cookie.as_object_mut() {
                    object.remove("sameSite");
                }
                let _ = this
                    .command("POST", "cookie", Some(json!({"cookie":cookie})))
                    .await;
            }
            this.goto(provider.login()).await?;
            if !this.authenticated().await.unwrap_or(false) {
                this.command("DELETE", "cookie", None).await?;
                this.goto(provider.login()).await?;
            }
        }
        Ok(this)
    }
    async fn command(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}/session/{}/{}", self.endpoint, self.session, path);
        let request = self
            .http
            .request(reqwest::Method::from_bytes(method.as_bytes())?, &url);
        let response = if let Some(body) = body {
            request.json(&body)
        } else {
            request
        }
        .send()
        .await
        .map_err(|_| anyhow!("Browser connection interrupted; reconnect this account"))?;
        let result: Value = response
            .json()
            .await
            .map_err(|_| anyhow!("Browser returned an invalid response"))?;
        if result["value"]["error"].is_string() {
            bail!("Browser operation interrupted. Keep the sign-in window open or reconnect.");
        }
        Ok(result["value"].clone())
    }
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.command("POST", "url", Some(json!({"url":url})))
            .await?;
        Ok(())
    }
    pub async fn cookies(&self) -> Result<Vec<Value>> {
        if let Some(native) = &self.native {
            return native.cookies();
        }
        Ok(self
            .command("GET", "cookie", None)
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }
    async fn verify_origin(&self) -> Result<()> {
        let current = self.command("GET", "url", None).await?;
        let url = reqwest::Url::parse(current.as_str().unwrap_or_default())
            .map_err(|_| anyhow!("Return to the official service page or reconnect"))?;
        if url.origin().ascii_serialization() != self.provider.origin() {
            bail!("The sign-in window left the official service. Reconnect before continuing");
        }
        Ok(())
    }
    pub async fn fetch(&self, path: &str, payload: Option<Value>) -> Result<WebResponse> {
        if let Some(native) = &self.native {
            return native.fetch(path, payload).await;
        }
        self.verify_origin().await?;
        if !path.starts_with('/') || path.starts_with("//") {
            bail!("Only service-relative requests are allowed");
        }
        let cookies = if payload.is_some() {
            self.cookies().await?
        } else {
            Vec::new()
        };
        let csrf = cookies
            .iter()
            .find(|c| c["name"] == "csrf_cookie")
            .and_then(|c| c["value"].as_str())
            .unwrap_or_default();
        let script = r#"
          const [path, payload, csrf] = arguments;
          const done = arguments[arguments.length - 1];
          const abort = new AbortController(); const timer = setTimeout(() => abort.abort(), 20000);
          const options = {credentials:'same-origin',signal:abort.signal};
          if (payload !== null) {
            options.method = 'POST'; options.body = new URLSearchParams(payload);
            options.headers = {'csrf-prevention-token':csrf};
          }
          fetch(path, options).then(async response => {
            const text = await response.text(); let body; try {body = JSON.parse(text)} catch (_) {body = text}
            const retry = response.headers.get('retry-after');
            const delay = retry === null ? null : /^\d+$/.test(retry.trim()) ? Number(retry) : Math.max(0, Math.ceil((Date.parse(retry) - Date.now()) / 1000));
            done({status:response.status,body,redirected:response.redirected,retry_after:Number.isFinite(delay) ? delay : null});
          }).catch(() => done({status:0,body:null})).finally(() => clearTimeout(timer));
        "#;
        let value = self
            .command(
                "POST",
                "execute/async",
                Some(json!({"script":script,"args":[path,payload,csrf]})),
            )
            .await?;
        serde_json::from_value(value).map_err(|_| anyhow!("Service response interrupted"))
    }
    pub async fn authenticated(&self) -> Result<bool> {
        let path = match self.provider {
            Provider::Humble => "/api/v1/user/order",
            Provider::Steam => "/account/registerkey",
        };
        let response = self.fetch(path, None).await?;
        if response.status == 403 {
            bail!(
                "The service blocked its authentication check. Complete any browser security step manually."
            );
        }
        Ok(response.status == 200
            && if self.provider == Provider::Steam {
                verified_steam_identity(&response.body, &self.cookies().await?)
            } else {
                !response.redirected && response.body.is_array()
            })
    }
    pub async fn fetch_many(&self, paths: &[String]) -> Result<Vec<WebResponse>> {
        if self.native.is_some() {
            return futures::future::try_join_all(paths.iter().map(|p| self.fetch(p, None))).await;
        }
        self.verify_origin().await?;
        if paths
            .iter()
            .any(|p| !p.starts_with('/') || p.starts_with("//"))
        {
            bail!("Invalid service-relative path");
        }
        let script = r#"
          const paths = arguments[0], done = arguments[arguments.length - 1];
          Promise.all(paths.map(async path => {
            const abort = new AbortController(); const timer = setTimeout(() => abort.abort(), 20000);
            try { const r = await fetch(path,{credentials:'same-origin',signal:abort.signal});
              const text = await r.text(); let body; try {body=JSON.parse(text)} catch (_) {body=text}
              return {status:r.status,body,redirected:r.redirected};
            } catch (_) {return {status:0,body:null}} finally {clearTimeout(timer)}
          })).then(done);
        "#;
        let response = self
            .command(
                "POST",
                "execute/async",
                Some(json!({"script":script,"args":[paths]})),
            )
            .await?;
        serde_json::from_value(response)
            .map_err(|_| anyhow!("Incomplete library response; retry synchronization"))
    }
}
fn verified_steam_identity(body: &Value, cookies: &[Value]) -> bool {
    let Some(html) = body.as_str() else {
        return false;
    };
    let pattern = regex::Regex::new(r#"\bg_steamID\s*=\s*["']([0-9]{17})["']"#).unwrap();
    let Some(identity) = pattern.captures(html) else {
        return false;
    };
    cookies.iter().any(|cookie| {
        cookie["name"] == "steamLoginSecure"
            && cookie["value"].as_str().is_some_and(|value| {
                urlencoding::decode(value)
                    .is_ok_and(|decoded| decoded.split("||").next() == Some(&identity[1]))
            })
    })
}
impl Drop for Browser {
    fn drop(&mut self) {
        use std::io::Write;
        if self.child.is_some()
            && !self.session.is_empty()
            && let Some(address) = self.endpoint.strip_prefix("http://")
            && let Ok(mut stream) = std::net::TcpStream::connect(address)
        {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let _ = write!(
                stream,
                "DELETE /session/{} HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                self.session, address
            );
            let mut response = [0; 512];
            let _ = stream.read(&mut response);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_archive_without_expected_binary() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            zip.start_file("unrelated", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.finish().unwrap();
        }
        assert!(unpack_driver(bytes.get_ref(), "chromedriver", true).is_err());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::fixtures::Fixture;
    #[test]
    fn steam_sign_in_requires_the_server_identity_to_match_the_cookie() {
        let cookies = vec![
            json!({"name":"steamLoginSecure","value":"12345678901234567%7C%7Csynthetic-token"}),
        ];
        assert!(verified_steam_identity(
            &json!("<script>g_steamID = \"12345678901234567\";</script>"),
            &cookies
        ));
        assert!(!verified_steam_identity(
            &json!("<script>g_steamID = false;</script>Sign in"),
            &cookies
        ));
        assert!(!verified_steam_identity(
            &json!("<script>g_steamID = \"12345678901234568\";</script>"),
            &cookies
        ));
        assert!(!verified_steam_identity(
            &json!("<script>g_steamID = \"12345678901234567\";</script>"),
            &[]
        ));
    }
    #[test]
    fn driver_must_match_browser_build() {
        assert!(matching_build("140.0.7339.82", "140.0.7339"));
        assert!(!matching_build("139.0.7258.10", "140.0.7339"));
    }
    #[tokio::test]
    async fn auth_rejects_login_redirect_and_malformed_humble_response() {
        for response in [
            json!({"status":200,"body":[],"redirected":true}),
            json!({"status":200,"body":{}}),
            json!({"status":401,"body":[]}),
        ] {
            let fixture = Fixture::service(Provider::Humble, response).await;
            assert!(
                !fixture
                    .browser
                    .as_ref()
                    .unwrap()
                    .authenticated()
                    .await
                    .unwrap()
            );
        }
        let fixture = Fixture::service(Provider::Humble, json!({"status":200,"body":[]})).await;
        assert!(
            fixture
                .browser
                .as_ref()
                .unwrap()
                .authenticated()
                .await
                .unwrap()
        );
    }
    #[tokio::test]
    async fn service_origin_and_security_blocks_fail_closed() {
        let fixture = Fixture::new(Provider::Steam, |path, _| {
            if path == "url" {
                json!("https://example.invalid")
            } else {
                panic!("No service request may be made off origin")
            }
        })
        .await;
        assert!(
            fixture
                .browser
                .as_ref()
                .unwrap()
                .fetch(
                    "/account/ajaxregisterkey/",
                    Some(json!({"product_key":"AAAAA-BBBBB-CCCCC"}))
                )
                .await
                .is_err()
        );
        let fixture = Fixture::service(
            Provider::Humble,
            json!({"status":403,"body":"Security challenge"}),
        )
        .await;
        assert!(
            fixture
                .browser
                .as_ref()
                .unwrap()
                .authenticated()
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn reject_external_service_paths() {
        let fixture = Fixture::service(Provider::Steam, Value::Null).await;
        for path in ["https://example.invalid", "//example.invalid/key"] {
            assert!(
                fixture
                    .browser
                    .as_ref()
                    .unwrap()
                    .fetch(path, None)
                    .await
                    .is_err()
            );
        }
    }
}
