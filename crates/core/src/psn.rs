//! Public catalog discovery and human-operated PlayStation checklists.
//! This module never authenticates HTTP requests or submits a claim/checkout.
use crate::{model::now, privacy::safe_text, store::Store};
use anyhow::{Result, anyhow, bail};
use reqwest::{Client, Url};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Monthly,
    Giveaway,
    FreeToPlay,
    Dlc,
    Pack,
    Demo,
    Trial,
    Unknown,
}
impl Category {
    pub const ALL: [Self; 8] = [
        Self::Monthly,
        Self::Giveaway,
        Self::FreeToPlay,
        Self::Dlc,
        Self::Pack,
        Self::Demo,
        Self::Trial,
        Self::Unknown,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Monthly => "PS Plus monthly",
            Self::Giveaway => "Free-to-keep offer",
            Self::FreeToPlay => "Free to play",
            Self::Dlc => "Free DLC",
            Self::Pack => "Promotional pack",
            Self::Demo => "Demo",
            Self::Trial => "Trial",
            Self::Unknown => "Unknown category",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub region: String,
    pub title: String,
    pub edition: Option<String>,
    pub category: Category,
    pub platforms: Vec<String>,
    pub price: Option<String>,
    pub subscription: Option<bool>,
    pub base_game: Option<bool>,
    pub entitlement: String,
    pub deadline: Option<i64>,
    pub url: String,
    pub source: String,
    pub checked_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub region: String,
    pub notifications: bool,
    pub next_refresh: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    pub url: String,
    pub last_attempt: i64,
    pub last_success: Option<i64>,
    pub count: usize,
    pub issue: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confirmation {
    Claimed,
    AlreadyOwned,
    Skipped,
    CouldNotClaim,
}
impl Confirmation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claimed => "Claimed",
            Self::AlreadyOwned => "Already owned",
            Self::Skipped => "Skipped",
            Self::CouldNotClaim => "Couldn’t claim",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub id: String,
    pub checklist_id: String,
    pub offer_id: String,
    pub region: String,
    pub title: String,
    pub outcome: Confirmation,
    pub evidence: String,
    pub at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checklist {
    pub id: String,
    pub region: String,
    pub offer_ids: Vec<String>,
    pub created_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenRequest {
    pub id: String,
    pub checklist_id: String,
    pub offer_id: String,
    pub url: String,
    pub requested_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct State {
    pub config: Config,
    pub offers: Vec<Offer>,
    pub checklists: Vec<Checklist>,
    pub history: Vec<ClaimRecord>,
    pub sources: Vec<Source>,
    pub open_request: Option<OpenRequest>,
    pub notification_issue: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    Track {
        url: String,
        title: String,
        category: Category,
    },
    Configure {
        region: String,
        notifications: bool,
    },
    Refresh,
    Plan {
        offer_ids: Vec<String>,
    },
    Open {
        checklist_id: String,
        offer_id: String,
    },
    // Native GUI only. Deliberately not represented by CLI commands or MCP schemas.
    Confirm {
        request_id: String,
        outcome: Confirmation,
    },
    BrowserSession {
        request_id: String,
    },
    SaveBrowserSession {
        request_id: String,
        cookies: Vec<Value>,
    },
    Close {
        request_id: String,
    },
    ForgetSession,
}
fn load_rows<T: serde::de::DeserializeOwned>(store: &Store, table: &str) -> Result<Vec<T>> {
    let mut q = store
        .db
        .prepare(&format!("SELECT metadata FROM {table} ORDER BY rowid"))?;
    q.query_map([], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect()
}
fn put_row<T: Serialize>(store: &Store, table: &str, id: &str, item: &T) -> Result<()> {
    store.db.execute(
        &format!("INSERT OR REPLACE INTO {table} (id,metadata) VALUES (?1,?2)"),
        rusqlite::params![id, serde_json::to_string(item)?],
    )?;
    Ok(())
}
impl State {
    pub fn load(store: &Store, demo: bool) -> Result<Self> {
        store.db.execute_batch("CREATE TABLE IF NOT EXISTS psn_offers(id TEXT PRIMARY KEY,metadata TEXT NOT NULL); CREATE TABLE IF NOT EXISTS psn_checklists(id TEXT PRIMARY KEY,metadata TEXT NOT NULL); CREATE TABLE IF NOT EXISTS psn_history(id TEXT PRIMARY KEY,metadata TEXT NOT NULL);")?;
        let mut state = Self {
            config: store
                .get_setting("psn-config")?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            offers: load_rows(store, "psn_offers")?,
            checklists: load_rows(store, "psn_checklists")?,
            history: load_rows(store, "psn_history")?,
            sources: store
                .get_setting("psn-sources")?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default(),
            ..Default::default()
        };
        if demo && state.offers.is_empty() {
            state.config.region = "en-us".into();
            for (i, category) in Category::ALL.into_iter().enumerate() {
                let offer = Offer {
                    id: format!("psn-demo-{i}"),
                    region: "en-us".into(),
                    title: format!("Synthetic PlayStation {}", category.label()),
                    edition: Some("Synthetic edition".into()),
                    category,
                    platforms: vec!["PS4".into(), "PS5".into()],
                    price: Some("Free — synthetic fixture".into()),
                    subscription: Some(category == Category::Monthly),
                    base_game: Some(matches!(category, Category::Dlc | Category::Pack)),
                    entitlement: entitlement(category, Some(category == Category::Monthly)).into(),
                    deadline: (i == 0).then(|| now() + 2 * 86400),
                    url: format!("https://store.playstation.com/en-us/product/SYNTHETIC-DEMO-{i}"),
                    source: "Synthetic fixture".into(),
                    checked_at: now(),
                };
                put_row(store, "psn_offers", &offer.id, &offer)?;
                state.offers.push(offer);
            }
        }
        Ok(state)
    }
    pub fn public(&self) -> Self {
        let mut s = self.clone();
        if s.open_request
            .as_ref()
            .is_some_and(|r| now() - r.requested_at > 1800)
        {
            s.open_request = None;
        }
        s
    }
    pub async fn handle(
        &mut self,
        store: &mut Store,
        command: Command,
        demo: bool,
    ) -> Result<Option<Value>> {
        match command {
            Command::Track {
                url,
                title,
                category,
            } => {
                validate_region(&self.config.region)?;
                let mut url = validate_public_url(&url)?;
                if url.path().split('/').nth(1) != Some(self.config.region.as_str())
                    || !(url.path().contains("/product/")
                        || url.path().contains("/concept/")
                        || url.path().contains("/games/"))
                {
                    bail!("Use an official product or game page for your chosen store locale");
                }
                let title = safe_text(title.trim());
                if title.is_empty() || title.len() > 200 {
                    bail!("Enter a game title of at most 200 characters");
                }
                url.set_query(None);
                url.set_fragment(None);
                let id = offer_id(&self.config.region, url.as_str());
                self.upsert(store,Offer{id,region:self.config.region.clone(),title,edition:None,category,platforms:vec![],price:None,subscription:None,base_game:None,entitlement:"User-added link; verify price, edition, requirements and entitlement on PlayStation".into(),deadline:None,url:url.to_string(),source:"User-added official link; eligibility unverified".into(),checked_at:now()})?;
            }
            Command::Configure {
                region,
                notifications,
            } => {
                if self.open_request.is_some() {
                    bail!("Close the current PlayStation claim window before changing settings");
                }
                validate_region(&region)?;
                let changed = self.config.region != region;
                self.config.region = region;
                self.config.notifications = notifications;
                if changed {
                    self.config.next_refresh = store
                        .get_setting("psn-cooldown-until")?
                        .and_then(|s| s.parse::<i64>().ok())
                        .filter(|t| *t > now());
                }
                self.open_request = None;
                self.save_config(store)?;
            }
            Command::Refresh => {
                if !demo {
                    self.refresh(store).await?;
                }
            }
            Command::Plan { mut offer_ids } => {
                validate_region(&self.config.region)?;
                offer_ids.sort();
                offer_ids.dedup();
                if offer_ids.is_empty() || offer_ids.len() > 500 {
                    bail!("Select between 1 and 500 offers");
                }
                for id in &offer_ids {
                    let o = self.offer(id)?;
                    if o.region != self.config.region {
                        bail!("Use offers from the selected store region");
                    }
                    if o.deadline.is_some_and(|t| t <= now()) {
                        bail!("An offer has expired; refresh and review the selection");
                    }
                }
                let list = Checklist {
                    id: uuid::Uuid::new_v4().to_string(),
                    region: self.config.region.clone(),
                    offer_ids,
                    created_at: now(),
                };
                put_row(store, "psn_checklists", &list.id, &list)?;
                self.checklists.push(list);
            }
            Command::Open {
                checklist_id,
                offer_id,
            } => {
                if let Some(open) = self
                    .open_request
                    .as_ref()
                    .filter(|r| now() - r.requested_at <= 1800)
                {
                    if open.checklist_id == checklist_id && open.offer_id == offer_id {
                        return Ok(None);
                    }
                    bail!(
                        "Finish or close the current PlayStation page before opening another item"
                    );
                }
                let list = self
                    .checklists
                    .iter()
                    .find(|l| l.id == checklist_id)
                    .ok_or_else(|| anyhow!("Checklist not found"))?;
                if list.region != self.config.region || !list.offer_ids.contains(&offer_id) {
                    bail!("Review the checklist for the selected region");
                }
                let offer = self.offer(&offer_id)?;
                validate_public_url(&offer.url)?;
                if offer.deadline.is_some_and(|t| t <= now()) {
                    bail!("This offer has expired; refresh its public listing");
                }
                self.open_request = Some(OpenRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    checklist_id,
                    offer_id,
                    url: offer.url.clone(),
                    requested_at: now(),
                });
            }
            Command::Confirm {
                request_id,
                outcome,
            } => {
                let r = self.request(&request_id)?.clone();
                let o = self.offer(&r.offer_id)?;
                let record = ClaimRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    checklist_id: r.checklist_id,
                    offer_id: o.id.clone(),
                    region: o.region.clone(),
                    title: o.title.clone(),
                    outcome,
                    evidence:
                        "User-confirmed; PlayStation ownership was not independently verified"
                            .into(),
                    at: now(),
                };
                put_row(store, "psn_history", &record.id, &record)?;
                self.history.push(record);
                self.open_request = None;
            }
            Command::BrowserSession { request_id } => {
                self.request(&request_id)?;
                let cookies = store
                    .secret::<Vec<Value>>(&session_key(&self.config.region))?
                    .unwrap_or_default();
                return Ok(Some(
                    json!({"psn_browser_session":true,"request_id":request_id,"cookies":cookies}),
                ));
            }
            Command::SaveBrowserSession {
                request_id,
                cookies,
            } => {
                self.request(&request_id)?;
                validate_cookies(&cookies)?;
                store.put_secret(&session_key(&self.config.region), &cookies)?;
            }
            Command::Close { request_id } => {
                self.request(&request_id)?;
                self.open_request = None;
            }
            Command::ForgetSession => {
                store.forget(&session_key(&self.config.region))?;
                self.open_request = None;
            }
        }
        Ok(None)
    }
    fn request(&self, id: &str) -> Result<&OpenRequest> {
        self.open_request
            .as_ref()
            .filter(|r| r.id == id && now() - r.requested_at <= 1800)
            .ok_or_else(|| anyhow!("Open this checklist item in the app again"))
    }
    pub fn offer(&self, id: &str) -> Result<&Offer> {
        self.offers
            .iter()
            .find(|o| o.id == id)
            .ok_or_else(|| anyhow!("Offer no longer available; refresh the inbox"))
    }
    fn save_config(&self, store: &Store) -> Result<()> {
        store.setting("psn-config", &serde_json::to_string(&self.config)?)
    }
    pub async fn tick(&mut self, store: &mut Store, demo: bool) -> Result<()> {
        if demo || !self.config.notifications {
            return Ok(());
        }
        if store
            .get_setting("psn-notify-check")?
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|t| now() - t < 3600)
        {
            return Ok(());
        }
        store.setting("psn-notify-check", &now().to_string())?;
        for offer in &self.offers {
            if offer.region != self.config.region
                || self.history.iter().any(|r| {
                    r.offer_id == offer.id
                        && matches!(
                            r.outcome,
                            Confirmation::Claimed | Confirmation::AlreadyOwned
                        )
                })
            {
                continue;
            }
            let Some(end) = offer
                .deadline
                .filter(|t| *t > now() && *t - now() <= 72 * 3600)
            else {
                continue;
            };
            let stage = if end - now() <= 86400 { 24 } else { 72 };
            let marker = format!("psn-notified:{}:{end}:{stage}", offer.id);
            if store.get_setting(&marker)?.is_some() {
                continue;
            }
            let title = safe_text(&offer.title);
            let sent = tokio::task::spawn_blocking(move || {
                notify_rust::Notification::new()
                    .summary("PlayStation offer ending soon")
                    .body(&format!(
                        "{title}: check its official page before the claim deadline."
                    ))
                    .appname("Humble Redeemer")
                    .show()
                    .map(|_| ())
                    .map_err(|_| ())
            })
            .await;
            if matches!(sent, Ok(Ok(()))) {
                store.setting(&marker, "sent")?;
                self.notification_issue = None;
            } else {
                self.notification_issue=Some("Desktop notifications are unavailable or were not authorized. Deadlines remain visible in the inbox.".into());
                break;
            }
        }
        Ok(())
    }
    pub async fn refresh(&mut self, store: &mut Store) -> Result<()> {
        validate_region(&self.config.region)?;
        if store
            .get_setting("psn-cooldown-until")?
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|t| t > now())
        {
            bail!(
                "PlayStation requested a public catalog cooldown; cached offers remain available"
            );
        }
        if self.config.next_refresh.is_some_and(|t| t > now()) {
            bail!("Public catalog refresh is cooling down; the cached inbox is still available");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent(concat!(
                "HumbleRedeemer/",
                env!("CARGO_PKG_VERSION"),
                " (public PlayStation catalog; no account access)"
            ))
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() < 5 && validate_public_url(a.url().as_str()).is_ok() {
                    a.follow()
                } else {
                    a.stop()
                }
            }))
            .build()?;
        let region = self.config.region.clone();
        let roots = source_roots(&region);
        let mut seen = HashSet::new();
        let mut total = 0;
        let budget = tokio::time::Instant::now() + Duration::from_secs(45);
        self.config.next_refresh = Some(now() + 900);
        self.save_config(store)?;
        for (name, url, category) in roots {
            let old = self.sources.iter().find(|s| s.url == url).cloned();
            let mut source = Source {
                name: name.into(),
                url: url.clone(),
                last_attempt: now(),
                last_success: old.as_ref().and_then(|s| s.last_success),
                count: old.as_ref().map(|s| s.count).unwrap_or(0),
                issue: None,
            };
            let mut next = Some(url);
            let mut source_count = 0;
            let mut pages = 0;
            while let Some(url) = next.take() {
                if tokio::time::Instant::now() >= budget {
                    source.issue=Some("Refresh time budget reached; public coverage is incomplete and cached offers were preserved".into());
                    break;
                }
                if pages >= 12 || !seen.insert(url.clone()) {
                    source.issue =
                        Some("Pagination limit or repeated page; coverage is incomplete".into());
                    break;
                }
                pages += 1;
                match read_public(&client, &url).await {
                    Ok((html, None)) => {
                        let parsed = parse_page(&html, &url, &region, category, now())?;
                        if parsed.offers.is_empty() {
                            source.issue=Some("The public page did not expose usable offer data. Open its official source to browse; cached offers were preserved.".into());
                            break;
                        }
                        source_count += parsed.offers.len();
                        for o in parsed.offers {
                            self.upsert(store, o)?;
                            total += 1;
                        }
                        next = parsed.next;
                    }
                    Ok((_, Some(retry))) => {
                        self.config.next_refresh = Some(now() + retry.max(900));
                        store.setting(
                            "psn-cooldown-until",
                            &self.config.next_refresh.unwrap().to_string(),
                        )?;
                        source.issue = Some(
                            "PlayStation requested a cooldown; cached offers were preserved".into(),
                        );
                        break;
                    }
                    Err(_) => {
                        source.issue =
                            Some("Public source unavailable; cached offers were preserved".into());
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            if source.issue.is_none() {
                source.last_success = Some(now());
                source.count = source_count;
            }
            let limited = store
                .get_setting("psn-cooldown-until")?
                .and_then(|s| s.parse::<i64>().ok())
                .is_some_and(|t| t > now());
            self.sources.retain(|s| s.url != source.url);
            self.sources.push(source);
            if limited {
                break;
            }
        }
        if total > 0 && self.sources.iter().all(|s| s.issue.is_none()) {
            self.config.next_refresh = Some(now() + 12 * 3600);
        }
        self.save_config(store)?;
        store.setting("psn-sources", &serde_json::to_string(&self.sources)?)?;
        Ok(())
    }
    fn upsert(&mut self, store: &Store, offer: Offer) -> Result<()> {
        put_row(store, "psn_offers", &offer.id, &offer)?;
        self.offers.retain(|o| o.id != offer.id);
        self.offers.push(offer);
        Ok(())
    }
}
fn session_key(region: &str) -> String {
    format!("psn-browser:{region}")
}
pub fn validate_region(region: &str) -> Result<()> {
    if !regex::Regex::new(r"^[a-z]{2}-[a-z]{2}$")
        .unwrap()
        .is_match(region)
    {
        bail!("Choose a store locale such as en-us, en-gb, en-ca, de-de or ja-jp");
    }
    Ok(())
}
pub fn sony_host(host: &str) -> bool {
    [
        "playstation.com",
        "sonyentertainmentnetwork.com",
        "account.sony.com",
    ]
    .iter()
    .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}
pub fn validate_navigation(url: &str) -> bool {
    Url::parse(url).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str().is_some_and(sony_host)
    })
}
pub fn validate_public_url(url: &str) -> Result<Url> {
    let u = Url::parse(url)?;
    if !validate_navigation(url)
        || !matches!(
            u.host_str(),
            Some("www.playstation.com" | "store.playstation.com")
        )
    {
        bail!("Only official public PlayStation catalog pages are accepted");
    }
    Ok(u)
}
pub fn validate_cookies(cookies: &[Value]) -> Result<()> {
    if cookies.len() > 2000 || serde_json::to_vec(cookies)?.len() > 2_000_000 {
        bail!("Browser session is too large");
    }
    for c in cookies {
        if !c["domain"]
            .as_str()
            .is_some_and(|d| sony_host(d.trim_start_matches('.')))
            || c["name"].as_str().is_none()
            || c["value"].as_str().is_none()
        {
            bail!("Browser session contains an unsupported cookie origin");
        }
    }
    Ok(())
}
fn source_roots(region: &str) -> Vec<(&'static str, String, Category)> {
    vec![
        (
            "PS Plus monthly games",
            format!("https://www.playstation.com/{region}/ps-plus/whats-new/"),
            Category::Monthly,
        ),
        (
            "Free-to-play editorial",
            format!(
                "https://www.playstation.com/{region}/editorial/great-free-to-play-games-on-playstation-4/"
            ),
            Category::FreeToPlay,
        ),
        (
            "Store free-to-play catalog",
            format!(
                "https://store.playstation.com/{region}/category/30749d2c-738f-4c36-9d6b-7ed517cca9ee/1/"
            ),
            Category::FreeToPlay,
        ),
        (
            "PlayStation Plus packs",
            format!(
                "https://store.playstation.com/{region}/category/50140526-145d-46be-9e23-c1fee7290df2/1/"
            ),
            Category::Pack,
        ),
        (
            "Store deals and giveaways",
            format!("https://store.playstation.com/{region}/pages/deals"),
            Category::Giveaway,
        ),
        (
            "Free add-ons and demos",
            format!("https://store.playstation.com/{region}/pages/browse"),
            Category::Unknown,
        ),
    ]
}
async fn read_public(client: &Client, url: &str) -> Result<(String, Option<i64>)> {
    validate_public_url(url)?;
    let response = client.get(url).send().await?;
    if Url::parse(url)?.path_segments().and_then(|mut p| p.next())
        != response.url().path_segments().and_then(|mut p| p.next())
    {
        bail!("The public page changed store locale; cached offers were preserved");
    }
    if response.status().as_u16() == 429 {
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| {
                s.parse().ok().or_else(|| {
                    httpdate::parse_http_date(s)
                        .ok()
                        .and_then(|t| t.duration_since(std::time::SystemTime::now()).ok())
                        .map(|d| d.as_secs() as i64)
                })
            })
            .unwrap_or(3600);
        return Ok((String::new(), Some(retry)));
    }
    if !response.status().is_success() {
        bail!("Public catalog request failed");
    }
    if response.content_length().is_some_and(|n| n > 8_000_000) {
        bail!("Public catalog page is too large");
    }
    let bytes = response.bytes().await?;
    if bytes.len() > 8_000_000 {
        bail!("Public catalog page is too large");
    }
    Ok((String::from_utf8(bytes.to_vec())?, None))
}
fn entitlement(category: Category, subscription: Option<bool>) -> &'static str {
    if matches!(category, Category::Demo | Category::Trial) {
        return "Limited demo or trial access; verify its duration and subscription requirements";
    }
    if subscription == Some(true) || category == Category::Monthly {
        "Subscription-dependent access"
    } else {
        match category {
            Category::Giveaway => "Free-to-keep offer; confirm the exact terms on PlayStation",
            Category::FreeToPlay => "Free-to-play access; in-game purchases may exist",
            Category::Demo | Category::Trial => "Limited demo or trial access",
            _ => "Unknown; check the official product page",
        }
    }
}
pub struct ParsedPage {
    pub offers: Vec<Offer>,
    pub next: Option<String>,
}
pub fn parse_page(
    html: &str,
    source: &str,
    region: &str,
    hint: Category,
    checked_at: i64,
) -> Result<ParsedPage> {
    validate_region(region)?;
    let base = validate_public_url(source)?;
    let doc = Html::parse_document(html);
    let mut offers = HashMap::new();
    let scripts = Selector::parse("script").unwrap();
    for node in doc.select(&scripts) {
        if let Ok(v) = serde_json::from_str::<Value>(&node.inner_html()) {
            collect_json(&v, region, source, hint, checked_at, &mut offers);
        }
    }
    // Editorial cards are useful discovery leads, not verified prices or ownership.
    let anchors = Selector::parse("a[href]").unwrap();
    for a in doc.select(&anchors) {
        let Some(href) = a.value().attr("href") else {
            continue;
        };
        let Ok(url) = base.join(href) else { continue };
        if validate_public_url(url.as_str()).is_err() {
            continue;
        }
        let p = url.path();
        let candidate = p.contains("/product/")
            || p.contains("/concept/")
            || (url.host_str() == Some("www.playstation.com") && p.contains("/games/"));
        if !candidate
            || p.split('/').nth(1) != Some(region)
            || !(hint == Category::FreeToPlay || hint == Category::Monthly)
        {
            continue;
        }
        let heading = a
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find_map(|parent| {
                let h: Vec<_> = parent
                    .select(&Selector::parse("h2, h3, h4").unwrap())
                    .collect();
                (h.len() == 1).then(|| h[0].text().collect::<Vec<_>>().join(" "))
            });
        let title = heading
            .or_else(|| a.value().attr("aria-label").map(str::to_owned))
            .or_else(|| {
                a.select(&Selector::parse("img[alt]").unwrap())
                    .next()
                    .and_then(|i| i.value().attr("alt"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| a.text().collect::<Vec<_>>().join(" "));
        let title = safe_text(title.trim());
        if title.len() < 3
            || title.len() > 200
            || [
                "Find out more",
                "Learn more",
                "Play now",
                "Buy now",
                "View game",
                "Play for free",
                "Get this game",
                "Try this game",
            ]
            .iter()
            .any(|s| title.eq_ignore_ascii_case(s))
        {
            continue;
        }
        let action = a.text().collect::<String>().trim().to_lowercase();
        let mut category = hint;
        // The same fragment contains monthly games, trials and paid promotions.
        if hint == Category::Monthly {
            if action == "try this game" {
                category = Category::Trial;
            } else if action != "get this game" {
                continue;
            }
        }
        if hint == Category::Monthly
            && !a
                .ancestors()
                .filter_map(scraper::ElementRef::wrap)
                .any(|p| {
                    p.value()
                        .attr("class")
                        .is_some_and(|c| c.contains("wn-latest-monthly-games-content"))
                })
        {
            continue;
        }
        if hint == Category::FreeToPlay
            && !a
                .text()
                .collect::<String>()
                .to_lowercase()
                .contains("play for free")
        {
            continue;
        }
        let mut canonical = url;
        canonical.set_query(None);
        canonical.set_fragment(None);
        let id = offer_id(region, canonical.as_str());
        offers.entry(id.clone()).or_insert(Offer {
            id,
            region: region.into(),
            title,
            edition: None,
            category,
            platforms: vec![],
            price: None,
            subscription: (hint == Category::Monthly).then_some(true),
            base_game: None,
            entitlement: entitlement(category, None).into(),
            deadline: None,
            url: canonical.to_string(),
            source: source.into(),
            checked_at,
        });
    }
    let next = doc
        .select(&Selector::parse("a[rel=next]").unwrap())
        .next()
        .and_then(|a| a.value().attr("href"))
        .and_then(|s| base.join(s).ok())
        .filter(|u| {
            validate_public_url(u.as_str()).is_ok()
                && u.host_str() == base.host_str()
                && u.path().contains(region)
        })
        .map(|u| u.to_string());
    Ok(ParsedPage {
        offers: offers.into_values().collect(),
        next,
    })
}
fn offer_id(region: &str, url: &str) -> String {
    hex::encode(Sha256::digest(format!("{region}|{url}")))
}
fn collect_json(
    value: &Value,
    region: &str,
    source: &str,
    hint: Category,
    checked_at: i64,
    out: &mut HashMap<String, Offer>,
) {
    match value {
        Value::Array(a) => {
            for v in a {
                collect_json(v, region, source, hint, checked_at, out)
            }
        }
        Value::Object(m) => {
            let title = m
                .get("name")
                .or_else(|| m.get("title"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let url = m
                .get("url")
                .or_else(|| m.get("productUrl"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    m.get("productId")
                        .or_else(|| m.get("id"))
                        .and_then(Value::as_str)
                        .filter(|id| id.contains('-') && id.contains('_'))
                        .map(|id| format!("https://store.playstation.com/{region}/product/{id}"))
                });
            if let (Some(title), Some(url)) = (title, url) {
                let url = Url::parse(&url)
                    .map(|mut u| {
                        u.set_query(None);
                        u.set_fragment(None);
                        u.to_string()
                    })
                    .unwrap_or(url);
                let price = m
                    .get("price")
                    .filter(|p| p.is_object())
                    .or_else(|| m.get("offers"))
                    .unwrap_or(&Value::Null);
                let explicit_free = price["isFree"] == true
                    || price.get("price").is_some_and(zero_price)
                    || price.get("discountedPrice").is_some_and(zero_price);
                let subscription = price.get("isTiedToSubscription").and_then(Value::as_bool);
                let class = m
                    .get("storeDisplayClassification")
                    .or_else(|| m.get("category"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                let category = if class.contains("trial") {
                    Category::Trial
                } else if class.contains("demo") {
                    Category::Demo
                } else if class.contains("pack") {
                    Category::Pack
                } else if class.contains("add") || class.contains("dlc") {
                    Category::Dlc
                } else if class.contains("monthly") {
                    Category::Monthly
                } else if class.contains("free_to_play") || class.contains("free-to-play") {
                    Category::FreeToPlay
                } else {
                    hint
                };
                let region_matches = Url::parse(&url)
                    .ok()
                    .is_some_and(|u| u.path().split('/').nth(1) == Some(region));
                if validate_public_url(&url).is_ok()
                    && region_matches
                    && (explicit_free
                        || (category == Category::Monthly && class.contains("monthly"))
                        || (category == Category::Trial
                            && class.contains("trial")
                            && subscription == Some(true)))
                    && !m
                        .get("productType")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.eq_ignore_ascii_case("subscription"))
                    && !title.to_lowercase().contains("playstation plus premium")
                    && !title.to_lowercase().contains("playstation plus essential")
                {
                    let id = offer_id(region, &url);
                    let platforms = m
                        .get("platforms")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .filter(|s| matches!(*s, "PS4" | "PS5"))
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    let deadline = price
                        .get("priceValidUntil")
                        .or_else(|| m.get("endDate"))
                        .and_then(Value::as_str)
                        .and_then(|s| {
                            time::OffsetDateTime::parse(
                                s,
                                &time::format_description::well_known::Rfc3339,
                            )
                            .ok()
                        })
                        .map(|t| t.unix_timestamp());
                    let display = if explicit_free {
                        Some(format!(
                            "Free{}",
                            price["priceCurrency"]
                                .as_str()
                                .map(|c| format!(" ({c})"))
                                .unwrap_or_default()
                        ))
                    } else if subscription == Some(true) {
                        Some("Included with eligible subscription".into())
                    } else {
                        None
                    };
                    out.insert(
                        id.clone(),
                        Offer {
                            id,
                            region: region.into(),
                            title: safe_text(title),
                            edition: m.get("edition").and_then(Value::as_str).map(safe_text),
                            category,
                            platforms,
                            price: display,
                            subscription,
                            base_game: m.get("requiresBaseGame").and_then(Value::as_bool),
                            entitlement: entitlement(category, subscription).into(),
                            deadline,
                            url,
                            source: source.into(),
                            checked_at,
                        },
                    );
                }
            }
            for v in m.values() {
                collect_json(v, region, source, hint, checked_at, out);
            }
        }
        _ => {}
    }
}
fn zero_price(v: &Value) -> bool {
    v.as_f64() == Some(0.)
        || v.as_str()
            .is_some_and(|s| s.parse::<f64>().ok() == Some(0.))
}

pub fn format_time(value: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(value)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "Unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_store() -> (tempfile::TempDir, Store, State) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path().into(), true).unwrap();
        let p = State::load(&s, true).unwrap();
        (d, s, p)
    }
    #[test]
    fn regions_origins_and_cookie_boundaries_are_exact() {
        assert!(validate_region("en-us").is_ok());
        assert!(validate_region("US").is_err());
        assert!(validate_public_url("https://store.playstation.com/en-us/product/fixture").is_ok());
        for u in [
            "http://store.playstation.com/en-us/",
            "https://store.playstation.com.evil.test/en-us/",
            "https://store.playstation.com@evil.test/en-us/",
            "https://store.playstation.com:444/en-us/",
            "https://account.sony.com/login",
        ] {
            assert!(validate_public_url(u).is_err(), "{u}");
        }
        assert!(validate_navigation("https://ca.account.sony.com/login"));
        assert!(
            validate_cookies(&[
                json!({"domain":".playstation.com","name":"fixture","value":"synthetic-session"})
            ])
            .is_ok()
        );
        assert!(
            validate_cookies(&[
                json!({"domain":"evilplaystation.com","name":"fixture","value":"synthetic-session"})
            ])
            .is_err()
        );
    }
    #[test]
    fn prices_regions_editions_and_partial_data_are_not_invented() {
        let html = r#"<script type="application/json">{"products":[{"name":"Free fixture","id":"UP0000-FIXTURE_00-EXAMPLE","price":{"isFree":true,"isTiedToSubscription":false,"priceCurrency":"USD","priceValidUntil":"2026-12-01T00:00:00Z"},"platforms":["PS4","PS5"],"edition":"Fixture edition","category":"ADD_ON","requiresBaseGame":true},{"name":"Paid fixture","url":"https://store.playstation.com/en-us/product/paid","price":{"discountedPrice":"9.99"}},{"name":"Wrong region","url":"https://store.playstation.com/en-gb/product/wrong","price":{"isFree":true}},{"name":"Incomplete fixture","url":"https://store.playstation.com/en-us/product/unknown"}]}</script>"#;
        let page = parse_page(
            html,
            "https://store.playstation.com/en-us/pages/browse",
            "en-us",
            Category::Unknown,
            123,
        )
        .unwrap();
        assert_eq!(page.offers.len(), 1);
        let o = &page.offers[0];
        assert_eq!(o.category, Category::Dlc);
        assert_eq!(o.edition.as_deref(), Some("Fixture edition"));
        assert_eq!(o.base_game, Some(true));
        assert_eq!(o.subscription, Some(false));
        assert!(o.deadline.is_some());
        assert_eq!(o.checked_at, 123);
        assert!(!zero_price(&json!("$0.00")));
        assert!(!zero_price(&json!(null)));
        assert!(!zero_price(&json!(-1)));
        assert!(zero_price(&json!("0.00")));
    }
    #[test]
    fn monthly_cards_do_not_include_other_subscription_catalog_links() {
        let html = r#"<div class="cmp-experiencefragment--wn-latest-monthly-games-content"><div><h3>Monthly fixture</h3><a href="/en-us/games/monthly/">Get this game</a></div></div><div><h3>Catalog fixture</h3><a href="/en-us/games/catalog/">Get this game</a></div>"#;
        let p = parse_page(
            html,
            "https://www.playstation.com/en-us/ps-plus/whats-new/",
            "en-us",
            Category::Monthly,
            1,
        )
        .unwrap();
        assert_eq!(p.offers.len(), 1);
        assert_eq!(p.offers[0].title, "Monthly fixture");
        assert_eq!(p.offers[0].subscription, Some(true));
        assert!(p.offers[0].price.is_none());
        assert!(p.offers[0].deadline.is_none());
        assert!(p.offers[0].entitlement.contains("Subscription"));
    }
    #[test]
    fn editorial_titles_use_local_headings_and_never_button_labels() {
        let html = r#"<section><h2>Free games</h2><div><h4>First fixture</h4><div><a href="/en-us/games/first/">Play for free</a></div></div><div><h3>Second fixture</h3><a href="/en-us/games/second/">Play for free</a></div><div><a href="/en-us/games/untitled/">Play for free</a></div><div><h4>Other region</h4><a href="/en-gb/games/wrong/">Play for free</a></div></section>"#;
        let p = parse_page(
            html,
            "https://www.playstation.com/en-us/editorial/free-games/",
            "en-us",
            Category::FreeToPlay,
            1,
        )
        .unwrap();
        let mut titles: Vec<_> = p.offers.iter().map(|o| o.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(titles, ["First fixture", "Second fixture"]);
    }
    #[test]
    fn public_pagination_deduplicates_and_rejects_other_origins() {
        let html = r#"<script type="application/ld+json">[{"name":"One fixture","url":"https://store.playstation.com/en-us/product/fixture","offers":{"price":"0"}},{"name":"One fixture","url":"https://store.playstation.com/en-us/product/fixture","offers":{"price":"0"}}]</script><a rel="next" href="/en-us/category/fixture/2/">Next</a>"#;
        let p = parse_page(
            html,
            "https://store.playstation.com/en-us/category/fixture/1/",
            "en-us",
            Category::Giveaway,
            1,
        )
        .unwrap();
        assert_eq!(p.offers.len(), 1);
        assert_eq!(
            p.next.as_deref(),
            Some("https://store.playstation.com/en-us/category/fixture/2/")
        );
        let p = parse_page(
            &html.replace("/en-us/category/fixture/2/", "https://evil.test/en-us/"),
            "https://store.playstation.com/en-us/category/fixture/1/",
            "en-us",
            Category::Giveaway,
            1,
        )
        .unwrap();
        assert!(p.next.is_none());
    }
    #[tokio::test]
    async fn opening_and_saving_a_browser_never_confirm_a_claim_and_secrets_stay_private() {
        let (_d, mut store, mut state) = fixture_store();
        state
            .handle(
                &mut store,
                Command::Plan {
                    offer_ids: vec!["psn-demo-0".into()],
                },
                true,
            )
            .await
            .unwrap();
        let list = state.checklists[0].id.clone();
        state
            .handle(
                &mut store,
                Command::Open {
                    checklist_id: list.clone(),
                    offer_id: "psn-demo-0".into(),
                },
                true,
            )
            .await
            .unwrap();
        assert!(state.history.is_empty());
        let request = state.open_request.as_ref().unwrap().id.clone();
        state.handle(&mut store,Command::SaveBrowserSession{request_id:request.clone(),cookies:vec![json!({"domain":".playstation.com","name":"synthetic","value":"private-fixture-cookie"})]},true).await.unwrap();
        assert!(
            !serde_json::to_string(&state.public())
                .unwrap()
                .contains("private-fixture-cookie")
        );
        assert!(state.history.is_empty());
        state
            .handle(
                &mut store,
                Command::Confirm {
                    request_id: request.clone(),
                    outcome: Confirmation::Claimed,
                },
                true,
            )
            .await
            .unwrap();
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].evidence.contains("User-confirmed"));
        assert!(state.open_request.is_none());
        assert!(
            state
                .handle(
                    &mut store,
                    Command::Confirm {
                        request_id: request,
                        outcome: Confirmation::AlreadyOwned
                    },
                    true
                )
                .await
                .is_err()
        );
        let restored = State::load(&store, true).unwrap();
        assert_eq!(restored.history.len(), 1);
        assert_eq!(restored.checklists[0].id, list);
        assert!(restored.open_request.is_none());
        assert!(store.batches().unwrap().is_empty());
    }
    #[tokio::test]
    async fn expired_wrong_region_and_out_of_checklist_offers_cannot_open() {
        let (_d, mut store, mut state) = fixture_store();
        state.offers[0].deadline = Some(now() - 1);
        assert!(
            state
                .handle(
                    &mut store,
                    Command::Plan {
                        offer_ids: vec!["psn-demo-0".into()]
                    },
                    true
                )
                .await
                .is_err()
        );
        state
            .handle(
                &mut store,
                Command::Plan {
                    offer_ids: vec!["psn-demo-1".into()],
                },
                true,
            )
            .await
            .unwrap();
        let list = state.checklists[0].id.clone();
        assert!(
            state
                .handle(
                    &mut store,
                    Command::Open {
                        checklist_id: list.clone(),
                        offer_id: "psn-demo-2".into()
                    },
                    true
                )
                .await
                .is_err()
        );
        state.config.region = "en-gb".into();
        assert!(
            state
                .handle(
                    &mut store,
                    Command::Open {
                        checklist_id: list,
                        offer_id: "psn-demo-1".into()
                    },
                    true
                )
                .await
                .is_err()
        );
        assert!(state.history.is_empty());
    }
    #[tokio::test]
    async fn server_cooldowns_survive_region_changes_and_restarts() {
        let (_d, mut store, mut state) = fixture_store();
        let until = now() + 3600;
        store
            .setting("psn-cooldown-until", &until.to_string())
            .unwrap();
        state
            .handle(
                &mut store,
                Command::Configure {
                    region: "en-gb".into(),
                    notifications: false,
                },
                true,
            )
            .await
            .unwrap();
        assert_eq!(state.config.next_refresh, Some(until));
        let mut restored = State::load(&store, false).unwrap();
        assert!(restored.refresh(&mut store).await.is_err());
        assert_eq!(restored.offers.len(), 8);
        assert!(restored.history.is_empty());
    }
}

#[cfg(test)]
mod session_persistence_tests {
    use super::*;
    #[tokio::test]
    async fn encrypted_sessions_restore_only_for_an_explicitly_reopened_item() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().into(), true).unwrap();
        // Exercise the persistent encrypted path with a random test vault, without OS keyring access.
        store.vault.persistent = true;
        let mut state = State::load(&store, true).unwrap();
        state
            .handle(
                &mut store,
                Command::Configure {
                    region: "en-us".into(),
                    notifications: false,
                },
                true,
            )
            .await
            .unwrap();
        state
            .handle(
                &mut store,
                Command::Plan {
                    offer_ids: vec!["psn-demo-0".into()],
                },
                true,
            )
            .await
            .unwrap();
        let list = state.checklists[0].id.clone();
        state
            .handle(
                &mut store,
                Command::Open {
                    checklist_id: list.clone(),
                    offer_id: "psn-demo-0".into(),
                },
                true,
            )
            .await
            .unwrap();
        let request = state.open_request.as_ref().unwrap().id.clone();
        state.handle(&mut store,Command::SaveBrowserSession{request_id:request,cookies:vec![json!({"domain":"store.playstation.com","name":"synthetic","value":"private-session-fixture"})]},true).await.unwrap();
        let bytes: Vec<u8> = store
            .db
            .query_row(
                "SELECT ciphertext FROM secrets WHERE id=?1",
                [session_key("en-us")],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private-session-fixture"));
        let vault = std::mem::replace(&mut store.vault, crate::vault::Vault::memory());
        drop(store);
        let mut store = Store::open(directory.path().into(), true).unwrap();
        store.vault = vault;
        let mut state = State::load(&store, false).unwrap();
        assert!(state.open_request.is_none());
        assert!(
            !serde_json::to_string(&state.public())
                .unwrap()
                .contains("private-session-fixture")
        );
        assert!(
            state
                .handle(
                    &mut store,
                    Command::BrowserSession {
                        request_id: "stale".into()
                    },
                    true
                )
                .await
                .is_err()
        );
        state
            .handle(
                &mut store,
                Command::Open {
                    checklist_id: list,
                    offer_id: "psn-demo-0".into(),
                },
                true,
            )
            .await
            .unwrap();
        let request = state.open_request.as_ref().unwrap().id.clone();
        let saved = state
            .handle(
                &mut store,
                Command::BrowserSession {
                    request_id: request,
                },
                true,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved["cookies"][0]["value"], "private-session-fixture");
        assert!(state.history.is_empty());
    }
    #[test]
    fn mixed_editorial_content_keeps_trials_and_monthly_claims_separate() {
        let html = r#"<script type="application/json">[{"name":"Trial fixture","url":"https://www.playstation.com/en-us/games/trial/"},{"name":"Paid feature fixture","url":"https://www.playstation.com/en-us/games/paid/"}]</script><div class="cmp-experiencefragment--wn-latest-monthly-games-content"><div><h3>Monthly fixture</h3><a href="/en-us/games/monthly/">Get this game</a></div><div><h3>Trial fixture</h3><a href="/en-us/games/trial/">Try this game</a></div><div><h3>Paid feature fixture</h3><a href="/en-us/games/paid/">Find out more</a></div></div>"#;
        let page = parse_page(
            html,
            "https://www.playstation.com/en-us/ps-plus/whats-new/",
            "en-us",
            Category::Monthly,
            1,
        )
        .unwrap();
        assert_eq!(page.offers.len(), 2);
        assert_eq!(
            page.offers
                .iter()
                .filter(|o| o.category == Category::Monthly)
                .count(),
            1
        );
        assert_eq!(
            page.offers
                .iter()
                .filter(|o| o.category == Category::Trial)
                .count(),
            1
        );
        assert!(
            page.offers
                .iter()
                .find(|o| o.category == Category::Trial)
                .unwrap()
                .entitlement
                .contains("Limited")
        );
    }
}
