use crate::{
    browser::{Browser, available_browsers, provision_driver},
    model::*,
    privacy::{csv_safe, safe_text, valid_key},
    session::SavedSession,
    store::Store,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub struct Engine {
    pub store: Store,
    pub demo: bool,
    browsers: HashMap<Provider, Browser>,
    connections: HashMap<Provider, Connection>,
    pub message: String,
    pub shutdown: bool,
    owned: HashSet<u32>,
    owned_checked: bool,
    steam_identity: Option<String>,
    auth_retry_at: HashMap<Provider, i64>,
}
impl Engine {
    pub fn new(store: Store, demo: bool) -> Result<Self> {
        store.recover()?;
        let mut this = Self {
            store,
            demo,
            browsers: HashMap::new(),
            connections: HashMap::from([
                (Provider::Humble, Connection::default()),
                (Provider::Steam, Connection::default()),
            ]),
            message: "Connect your accounts to begin. Passwords stay on the providers' own pages."
                .into(),
            shutdown: false,
            owned: HashSet::new(),
            owned_checked: false,
            steam_identity: None,
            auth_retry_at: HashMap::new(),
        };
        if demo {
            this.seed_demo()?;
        } else {
            for provider in [Provider::Humble, Provider::Steam] {
                let saved = this
                    .store
                    .secret::<SavedSession>(&format!("native-session:{}", provider.name()))
                    .ok()
                    .flatten()
                    .or_else(|| {
                        this.store
                            .secret::<Vec<Value>>(&format!("session:{}", provider.name()))
                            .ok()
                            .flatten()
                            .map(|cookies| SavedSession {
                                cookies,
                                steam_account_id: None,
                                user_agent: concat!(
                                    "HumbleRedeemer/",
                                    env!("CARGO_PKG_VERSION"),
                                    " (unofficial desktop utility)"
                                )
                                .into(),
                            })
                    });
                if let Some(saved) = saved {
                    if provider == Provider::Steam {
                        this.steam_identity = saved
                            .steam_account_id
                            .clone()
                            .or_else(|| crate::browser::steam_account_id(&saved.cookies));
                    }
                    let browser = Browser::from_saved(provider, saved)?;
                    this.browsers.insert(provider, browser);
                    this.connections.get_mut(&provider).unwrap().signing_in = true;
                    this.connections.get_mut(&provider).unwrap().message =
                        "Checking the saved session in the background…".into();
                }
            }
        }
        Ok(this)
    }
    fn seed_demo(&mut self) -> Result<()> {
        for provider in [Provider::Humble, Provider::Steam] {
            self.connections.insert(
                provider,
                Connection {
                    connected: true,
                    message: "Synthetic demo account".into(),
                    ..Default::default()
                },
            );
            self.store.setting(provider.name(), "demo-binding")?;
        }
        if self.store.games()?.is_empty() {
            for (index, outcome) in [
                Outcome::Ready,
                Outcome::Unrevealed,
                Outcome::AlreadyOwned,
                Outcome::UsedElsewhere,
                Outcome::Expired,
                Outcome::Exhausted,
            ]
            .into_iter()
            .enumerate()
            {
                self.store.put_game(&Game {
                    id: format!("demo-{index}"),
                    title: format!("Example adventure {}", index + 1),
                    app_id: Some(100 + index as u32),
                    source: "Synthetic fixtures".into(),
                    choice_month: (index < 2).then(|| "example-month".into()),
                    outcome,
                    reason: if outcome.eligible() {
                        "Demo item; no service requests will be made.".into()
                    } else {
                        outcome_reason(outcome).into()
                    },
                    service_code: None,
                    has_key: outcome != Outcome::Unrevealed,
                    updated_at: now(),
                })?;
            }
            self.store.put_choice(&Choice {
                id: "demo-choice".into(),
                month_id: "demo-month".into(),
                month: "Example month".into(),
                title: "Example Choice selection".into(),
                remaining: 1,
                available: true,
            })?;
        }
        self.message =
            "DEMO — synthetic data only. No real sign-ins, claims, or redemptions.".into();
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut steam = self.connections[&Provider::Steam].clone();
        steam.session_renewal_available = self
            .browsers
            .get(&Provider::Steam)
            .map(|b| b.native_snapshot())
            .transpose()?
            .flatten()
            .is_some_and(|s| s.cookies.iter().any(|c| c["name"] == "steamRefresh_steam"));
        Ok(Snapshot {
            version: env!("CARGO_PKG_VERSION").into(),
            demo: self.demo,
            persistent_vault: self.store.vault.persistent,
            steam_retry_at: self
                .store
                .get_setting("steam-cooldown-until")?
                .and_then(|s| s.parse().ok())
                .filter(|t| *t > now()),
            humble: self.connections[&Provider::Humble].clone(),
            steam,
            games: self.store.games()?,
            choices: self.store.choices()?,
            batches: self.store.batches()?,
            message: safe_text(&self.message),
            browsers: available_browsers(),
        })
    }
    fn pending_cooldown(&self) -> Result<Option<i64>> {
        let mut deadline = None;
        for service in ["steam", "humble"] {
            if let Some(t) = self
                .store
                .get_setting(&format!("{service}-cooldown-until"))?
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|t| *t > now())
            {
                deadline = Some(deadline.unwrap_or(t).max(t));
            }
        }
        Ok(deadline)
    }
    fn binding(&self) -> Result<String> {
        let h = self.store.get_setting("humble")?.unwrap_or_default();
        let s = self.store.get_setting("steam")?.unwrap_or_default();
        Ok(hex::encode(Sha256::digest(format!("{h}|{s}"))))
    }
    fn bind_session(&self, provider: Provider, binding: &str) -> Result<()> {
        if provider == Provider::Steam
            && self.store.get_setting("steam")?.as_deref() != Some(binding)
        {
            // An ownership result belongs to the account checked at that time.
            // Historical activations remain history, but a new session must
            // obtain fresh ownership evidence before skipping an unused key.
            for mut game in self.store.games()? {
                if game.outcome == Outcome::AlreadyOwned {
                    game.outcome = if game.has_key {
                        Outcome::Ready
                    } else {
                        Outcome::Unrevealed
                    };
                    game.reason =
                        "Ownership needs verification for the connected Steam session.".into();
                    self.store.put_game(&game)?;
                }
            }
        }
        self.store.setting(provider.name(), binding)
    }
    fn session_binding(&self, provider: Provider, cookies: &[Value]) -> String {
        if provider == Provider::Steam
            && let Some(identity) = crate::browser::steam_account_id(cookies)
        {
            // Preserve an existing approval across cookie rotation for the same
            // verified account, including sessions saved by earlier versions.
            if self.steam_identity.as_ref() == Some(&identity)
                && let Ok(Some(binding)) = self.store.get_setting("steam")
                && !binding.is_empty()
            {
                return binding;
            }
            return self.id("steam-account", &identity);
        }
        let mut identities: Vec<_> = cookies
            .iter()
            .map(|cookie| {
                json!([
                    cookie["name"],
                    cookie["domain"]
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches('.'),
                    cookie["path"],
                    cookie["value"]
                ])
                .to_string()
            })
            .collect();
        identities.sort();
        self.id(provider.name(), &identities.join("\n"))
    }
    fn id(&self, scope: &str, source: &str) -> String {
        self.store
            .vault
            .fingerprint(format!("{scope}:{source}").as_bytes())
    }
    fn connected(&self, provider: Provider) -> Result<&Browser> {
        if !self.connections[&provider].connected {
            bail!("Sign in to {} first", provider.name());
        }
        self.browsers
            .get(&provider)
            .ok_or_else(|| anyhow!("Reconnect {}", provider.name()))
    }
    pub async fn handle(&mut self, command: Command) -> Result<Value> {
        match command {
            Command::Snapshot => return Ok(serde_json::to_value(self.snapshot()?)?),
            Command::Connect { provider, browser } => {
                if self.demo {
                    return Ok(serde_json::to_value(self.snapshot()?)?);
                }
                if browser.is_none() {
                    self.browsers.remove(&provider);
                    self.connections.insert(provider, Connection { signing_in: true, native_login: true,
                        message: "Finish sign-in inside Humble Redeemer. No separate Chrome window is required.".into(), ..Default::default() });
                    self.message = "Open the app's sign-in panel to connect this account.".into();
                    return Ok(serde_json::to_value(self.snapshot()?)?);
                }
                if provider == Provider::Steam {
                    self.owned.clear();
                    self.owned_checked = false;
                }
                let saved = self
                    .store
                    .secret::<Vec<Value>>(&format!("session:{}", provider.name()))
                    .ok()
                    .flatten();
                self.browsers.remove(&provider);
                let driver = Browser::open(&self.store.root, provider, browser, saved).await?;
                self.connections.insert(
                    provider,
                    Connection {
                        connected: false,
                        signing_in: true,
                        native_login: false,
                        session_renewal_available: false,
                        browser: Some(driver.kind),
                        message: "Complete sign-in in the browser. This app will detect success."
                            .into(),
                    },
                );
                self.browsers.insert(provider, driver);
                self.message =
                    "Sign in directly on the official page; keep that browser window open.".into();
            }
            Command::AttachNativeSession {
                provider,
                cookies,
                user_agent,
            } => {
                let browser = Browser::from_native(provider, cookies, user_agent.clone())?;
                let (authenticated, diagnostic) = browser.authentication_report().await?;
                if !authenticated {
                    bail!(
                        "The provider did not verify this background session ({diagnostic}). Keep the sign-in page open and verify again."
                    );
                }
                let cookies = browser.cookies().await?;
                let binding = self.session_binding(provider, &cookies);
                self.store.put_secret(
                    &format!("native-session:{}", provider.name()),
                    &browser
                        .native_snapshot()?
                        .ok_or_else(|| anyhow!("Native session unavailable"))?,
                )?;
                self.bind_session(provider, &binding)?;
                if provider == Provider::Steam {
                    self.steam_identity =
                        crate::browser::steam_account_id(&browser.cookies().await?);
                }
                self.auth_retry_at.remove(&provider);
                self.browsers.insert(provider, browser);
                self.connections.insert(
                    provider,
                    Connection {
                        connected: true,
                        message: "Verified sign-in. Background work is windowless.".into(),
                        ..Default::default()
                    },
                );
                if provider == Provider::Steam {
                    self.owned.clear();
                    self.owned_checked = false;
                }
                self.message =
                    format!("{} connected. The sign-in view can close.", provider.name());
            }
            Command::Disconnect { provider } => {
                self.auth_retry_at.remove(&provider);
                if provider == Provider::Steam {
                    self.steam_identity = None;
                    self.owned.clear();
                    self.owned_checked = false;
                }
                self.browsers.remove(&provider);
                self.connections.insert(provider, Connection::default());
                self.store.forget(&format!("session:{}", provider.name()))?;
                self.store
                    .forget(&format!("native-session:{}", provider.name()))?;
                self.store.setting(provider.name(), "")?;
                for mut batch in self.store.batches()? {
                    if ["running", "cooldown"].contains(&batch.state.as_str()) {
                        batch.state = "paused".into();
                        batch.approved = false;
                        self.store.put_batch(&batch)?;
                    }
                }
                self.message =
                    "Local session forgotten. Reconnect before starting another batch.".into();
            }
            Command::Sync => {
                if !self.demo {
                    self.sync().await?;
                }
            }
            Command::Plan {
                game_ids,
                choice_ids,
                allow_reveal,
            } => {
                return Ok(serde_json::to_value(self.plan(
                    game_ids,
                    choice_ids,
                    allow_reveal,
                )?)?);
            }
            Command::Approve { batch_id, digest } => {
                let mut batch = self.store.batch(&batch_id)?;
                if batch.state != "review" {
                    bail!("This batch was already reviewed or ended; create a new plan");
                }
                self.validate_review(&batch)?;
                if batch.digest != digest || batch.binding != self.binding()? {
                    bail!("Batch changed or account connection changed. Review a new batch.");
                }
                batch.approved = true;
                batch.state = "approved".into();
                batch.message = "Approved for these exact items and accounts.".into();
                self.store.put_batch(&batch)?;
            }
            Command::Start { batch_id } | Command::Resume { batch_id } => {
                let mut batch = self.store.batch(&batch_id)?;
                self.validate_review(&batch)?;
                if !batch.approved || batch.binding != self.binding()? {
                    bail!(
                        "Review and approve this batch for the current account connections first"
                    );
                }
                if !self.demo && legacy_running() {
                    bail!(
                        "The legacy Python redeemer is still running. Pause or finish it before starting this worker."
                    );
                }
                if self.store.batches()?.iter().any(|b| {
                    b.id != batch.id && ["running", "cooldown"].contains(&b.state.as_str())
                }) {
                    bail!("Another batch is already active");
                }
                if ["complete", "cancelled", "needs_review", "claiming"]
                    .contains(&batch.state.as_str())
                {
                    bail!("This batch has ended or needs a new review; create a new plan");
                }
                if !self.connections[&Provider::Humble].connected
                    || !self.connections[&Provider::Steam].connected
                {
                    bail!("Connect both accounts before running this batch");
                }
                if let Some(deadline) = self.pending_cooldown()? {
                    batch.next_retry_at =
                        Some(batch.next_retry_at.unwrap_or(deadline).max(deadline));
                }
                batch.state = if batch.next_retry_at.is_some_and(|t| t > now()) {
                    "cooldown"
                } else {
                    "running"
                }
                .into();
                self.store.put_batch(&batch)?;
            }
            Command::Pause { batch_id } => {
                let mut batch = self.store.batch(&batch_id)?;
                if !["running", "cooldown", "approved", "paused"].contains(&batch.state.as_str()) {
                    bail!("Only active or approved batches can be paused");
                }
                batch.state = "paused".into();
                self.store.put_batch(&batch)?;
            }
            Command::Cancel { batch_id } => {
                let mut batch = self.store.batch(&batch_id)?;
                batch.state = "cancelled".into();
                batch.approved = false;
                self.store.put_batch(&batch)?;
            }
            Command::Retry { game_id } => {
                let mut game = self.store.game(&game_id)?;
                if matches!(
                    game.outcome,
                    Outcome::UnknownOutcome
                        | Outcome::UsedElsewhere
                        | Outcome::Expired
                        | Outcome::Redeemed
                        | Outcome::AlreadyOwned
                ) {
                    bail!("This outcome cannot be retried automatically");
                }
                game.outcome = if game.has_key {
                    Outcome::Ready
                } else {
                    Outcome::Unrevealed
                };
                game.reason = "Ready for a newly approved batch.".into();
                self.store.put_game(&game)?;
            }
            Command::Import { path } => {
                let count = self.import_csv(Path::new(&path))?;
                self.message = format!(
                    "Imported {count} local history records. Original files were not changed."
                );
            }
            Command::Export { path, include_keys } => {
                self.export(Path::new(&path), include_keys)?;
                self.message = "Report saved locally.".into();
            }
            Command::RevealKey { game_id } => {
                let private = self
                    .store
                    .secret::<PrivateGame>(&format!("game:{game_id}"))?
                    .ok_or_else(|| anyhow!("Key unavailable; synchronize this library again"))?;
                return Ok(json!({"key":private.key}));
            }
            Command::InstallDriver { browser } => {
                if !self.demo {
                    provision_driver(&self.store.root, browser).await?;
                }
                self.message =
                    "Browser driver is ready and its integrity receipt is saved privately.".into();
            }
            Command::Shutdown { pause } => {
                if pause {
                    for mut batch in self.store.batches()? {
                        if ["running", "cooldown"].contains(&batch.state.as_str()) {
                            batch.state = "paused".into();
                            self.store.put_batch(&batch)?;
                        }
                    }
                }
                self.shutdown = true;
            }
        }
        Ok(serde_json::to_value(self.snapshot()?)?)
    }
    pub fn plan(
        &self,
        mut game_ids: Vec<String>,
        mut choice_ids: Vec<String>,
        allow_reveal: bool,
    ) -> Result<Batch> {
        if !self.connections[&Provider::Humble].connected
            || !self.connections[&Provider::Steam].connected
        {
            bail!("Connect both accounts before planning a batch");
        }
        game_ids.sort();
        game_ids.dedup();
        choice_ids.sort();
        choice_ids.dedup();
        if game_ids.is_empty() && choice_ids.is_empty() {
            bail!("Select at least one game or Choice item");
        }
        let mut scope = Vec::new();
        for id in &game_ids {
            let g = self.store.game(id)?;
            if !g.outcome.eligible() {
                bail!("Selection includes an item that needs review or is already complete");
            }
            if !g.has_key && !allow_reveal {
                bail!("This selection includes unrevealed keys. Explicitly allow reveals first.");
            }
            scope.push(json!([g.id, g.title, g.app_id]));
        }
        let choices = self.store.choices()?;
        let mut per_month: HashMap<String, (usize, usize)> = HashMap::new();
        for id in &choice_ids {
            let c = choices.iter().find(|c| &c.id == id).ok_or_else(|| {
                anyhow!("Choice selection changed; synchronize and review it again")
            })?;
            if !c.available {
                bail!("This selection requires action on Humble and cannot be claimed by the app");
            }
            let count = per_month
                .entry(c.month_id.clone())
                .or_insert((0, c.remaining));
            count.0 += 1;
            if count.0 > count.1 {
                bail!("Too many selections for this Choice month");
            }
            scope.push(json!([c.id, c.title, c.month_id]));
        }
        if !choice_ids.is_empty() && !allow_reveal {
            bail!("Choice activation requires permission to reveal the selected keys");
        }
        let binding = self.binding()?;
        let id = uuid::Uuid::new_v4().to_string();
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
            id,
            scope,
            allow_reveal,
            binding
        ]))?));
        let batch = Batch {
            id,
            digest,
            reviewed_game_ids: game_ids.clone(),
            game_ids,
            choice_ids,
            allow_reveal,
            approved: false,
            state: "review".into(),
            cursor: 0,
            choice_cursor: 0,
            next_retry_at: None,
            binding,
            message: "Review the selected titles and reveal permission, then approve once.".into(),
        };
        self.store.put_batch(&batch)?;
        Ok(batch)
    }
    fn validate_review(&self, batch: &Batch) -> Result<()> {
        let mut scope = Vec::new();
        for id in &batch.reviewed_game_ids {
            let g = self.store.game(id)?;
            scope.push(json!([g.id, g.title, g.app_id]));
        }
        let choices = self.store.choices()?;
        for id in &batch.choice_ids {
            let c = choices
                .iter()
                .find(|c| &c.id == id)
                .ok_or_else(|| anyhow!("Choice selections changed; review a new batch"))?;
            scope.push(json!([c.id, c.title, c.month_id]));
        }
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
            batch.id,
            scope,
            batch.allow_reveal,
            self.binding()?
        ]))?));
        if digest != batch.digest {
            bail!("The reviewed items or account connections changed; review a new batch");
        }
        Ok(())
    }
    fn persist_native_sessions(&mut self) -> Result<()> {
        for provider in [Provider::Humble, Provider::Steam] {
            if let Some(saved) = self
                .browsers
                .get(&provider)
                .map(|b| b.native_snapshot())
                .transpose()?
                .flatten()
            {
                // A restored session was verified before it was originally
                // saved. Keep renewal rotations even if a later check had a
                // transient error; never discard its refresh cookie.
                let digest = self.store.vault.fingerprint(&serde_json::to_vec(&saved)?);
                let marker = format!("native-snapshot:{}", provider.name());
                if self.store.get_setting(&marker)?.as_deref() != Some(&digest) {
                    self.store
                        .put_secret(&format!("native-session:{}", provider.name()), &saved)?;
                    self.store.setting(&marker, &digest)?;
                }
            }
        }
        Ok(())
    }
    pub async fn tick(&mut self) -> Result<()> {
        if !self.demo {
            for provider in [Provider::Humble, Provider::Steam] {
                if self.connections[&provider].signing_in
                    && self
                        .auth_retry_at
                        .get(&provider)
                        .is_none_or(|t| *t <= now())
                {
                    let result = if let Some(browser) = self.browsers.get(&provider) {
                        browser.authenticated().await
                    } else {
                        Ok(false)
                    };
                    match result {
                        Ok(true) => {
                            let cookies = self.browsers[&provider].cookies().await?;
                            let binding = self.session_binding(provider, &cookies);
                            self.store
                                .put_secret(&format!("session:{}", provider.name()), &cookies)?;
                            self.bind_session(provider, &binding)?;
                            if provider == Provider::Steam {
                                self.steam_identity = crate::browser::steam_account_id(&cookies);
                            }
                            self.auth_retry_at.remove(&provider);
                            let c = self.connections.get_mut(&provider).unwrap();
                            c.connected = true;
                            c.signing_in = false;
                            c.native_login = false;
                            c.message = "Verified sign-in. Session stays on this computer.".into();
                            self.message = format!("{} is connected.", provider.name());
                        }
                        Ok(false) => {
                            let connection = self.connections.get_mut(&provider).unwrap();
                            if connection.browser.is_none() && self.browsers.contains_key(&provider)
                            {
                                connection.native_login = true;
                                connection.signing_in = false;
                                connection.message =
                                    "The provider requires sign-in again. Open the app's sign-in panel.".into();
                            }
                        }
                        Err(error) => {
                            let deadline = self
                                .browsers
                                .get(&provider)
                                .map(|b| b.native_auth_retry_at())
                                .unwrap_or(0)
                                .max(now() + 60);
                            self.auth_retry_at.insert(provider, deadline);
                            let connection = self.connections.get_mut(&provider).unwrap();
                            connection.message = safe_text(&error.to_string());
                        }
                    }
                }
            }
        }
        self.persist_native_sessions()?;
        let Some(mut batch) = self
            .store
            .batches()?
            .into_iter()
            .find(|b| ["running", "cooldown"].contains(&b.state.as_str()))
        else {
            return Ok(());
        };
        if !self.demo && legacy_running() {
            batch.state = "paused".into();
            batch.message =
                "The legacy redeemer is running. Finish or pause it before resuming this batch."
                    .into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if batch.binding != self.binding()? {
            batch.state = "paused".into();
            batch.approved = false;
            batch.message = "Account connections changed. Review a new batch.".into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if self.validate_review(&batch).is_err() {
            batch.state = "paused".into();
            batch.approved = false;
            batch.message =
                "Reviewed items changed during synchronization. Plan and review a new batch."
                    .into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if let Some(deadline) = self.pending_cooldown()? {
            batch.next_retry_at = Some(batch.next_retry_at.unwrap_or(deadline).max(deadline));
            batch.state = "cooldown".into();
            batch.message="A service cooldown is active. This deadline also applies to newly created batches.".into();
            self.store.put_batch(&batch)?;
        }
        if batch.next_retry_at.is_some_and(|t| t > now()) {
            return Ok(());
        }
        batch.state = "running".into();
        batch.next_retry_at = None;
        if self.demo {
            if batch.cursor < batch.game_ids.len() {
                let mut game = self.store.game(&batch.game_ids[batch.cursor])?;
                game.outcome = Outcome::Redeemed;
                game.reason = "Synthetic demo activation.".into();
                self.store.put_game(&game)?;
                batch.cursor += 1;
            } else {
                batch.choice_cursor = batch.choice_ids.len();
                batch.state = "complete".into();
            }
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if !self.connections[&Provider::Humble].connected
            || !self.connections[&Provider::Steam].connected
        {
            batch.state = "paused".into();
            batch.message = "Reconnect the account that needs sign-in.".into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if batch.choice_cursor < batch.choice_ids.len() {
            self.claim_choice(&mut batch).await?;
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if self
            .browsers
            .get(&Provider::Steam)
            .is_some_and(|b| b.is_native())
        {
            if self
                .auth_retry_at
                .get(&Provider::Steam)
                .is_some_and(|t| *t > now())
            {
                return Ok(());
            }
            // Verify/renew *before* persisting activation intent. A failed
            // renewal cannot create an uncertain activation or replay a POST.
            match self.browsers[&Provider::Steam].authenticated().await {
                Ok(true) => {
                    self.persist_native_sessions()?;
                }
                Ok(false) => {
                    let c = self.connections.get_mut(&Provider::Steam).unwrap();
                    c.connected = false;
                    c.signing_in = false;
                    c.native_login = true;
                    c.message =
                        "Steam requires sign-in again; the approved queue is paused.".into();
                    batch.state = "paused".into();
                    batch.message = c.message.clone();
                    self.store.put_batch(&batch)?;
                    return Ok(());
                }
                Err(error) => {
                    let deadline = self.browsers[&Provider::Steam]
                        .native_auth_retry_at()
                        .max(now() + 60);
                    self.auth_retry_at.insert(Provider::Steam, deadline);
                    batch.message = format!(
                        "Connection check will retry automatically: {}",
                        safe_text(&error.to_string())
                    );
                    self.persist_native_sessions()?;
                    self.store.put_batch(&batch)?;
                    return Ok(());
                }
            }
        }
        if !self.owned_checked {
            self.refresh_owned().await?;
        }
        if batch.cursor >= batch.game_ids.len() {
            batch.state = "complete".into();
            batch.message =
                "All approved items have been processed. Review individual outcomes.".into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        let mut game = self.store.game(&batch.game_ids[batch.cursor])?;
        if game.app_id.is_some_and(|id| self.owned.contains(&id)) {
            game.outcome = Outcome::AlreadyOwned;
            game.reason = outcome_reason(game.outcome).into();
            self.store.put_game(&game)?;
            batch.cursor += 1;
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if matches!(game.outcome, Outcome::UnknownOutcome | Outcome::InFlight) {
            batch.state = "paused".into();
            batch.message =
                "An earlier request has an uncertain outcome. Review it before continuing.".into();
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        let secret_id = format!("game:{}", game.id);
        let mut private = self
            .store
            .secret::<PrivateGame>(&secret_id)?
            .ok_or_else(|| anyhow!("Private item data unavailable; reconnect and synchronize"))?;
        if private.key.is_none() {
            if !batch.allow_reveal {
                bail!("Batch does not permit key reveals")
            }
            game.outcome = Outcome::InFlight;
            self.store.put_game(&game)?;
            let response=self.connected(Provider::Humble)?.fetch("/humbler/redeemkey",Some(json!({"keytype":private.machine,"key":private.order,"keyindex":private.index}))).await;
            let response = match response {
                Ok(r) => r,
                Err(_) => {
                    game.outcome = Outcome::UnknownOutcome;
                    game.reason =
                        "Reveal response was interrupted; synchronize before attempting it again."
                            .into();
                    self.store.put_game(&game)?;
                    batch.state = "paused".into();
                    self.store.put_batch(&batch)?;
                    return Ok(());
                }
            };
            if response.status == 401 || response.status == 403 {
                self.connections
                    .get_mut(&Provider::Humble)
                    .unwrap()
                    .connected = false;
                game.outcome = Outcome::LoginRequired;
                batch.state = "paused".into();
            } else if response.status == 429 {
                game.outcome = Outcome::RateLimited;
                batch.state = "cooldown".into();
                batch.next_retry_at = Some(cooldown_deadline(response.retry_after));
                self.store.setting(
                    "humble-cooldown-until",
                    &batch.next_retry_at.unwrap().to_string(),
                )?;
                batch.message =
                    "Humble requested a cooldown. This approved reveal will wait.".into();
            } else if let Some(key) = response.body["key"].as_str().filter(|k| valid_key(k)) {
                private.key = Some(key.into());
                game.has_key = true;
                self.store.put_secret(&secret_id, &private)?;
            } else {
                let msg = response.body["error_msg"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase();
                game.outcome = if msg.contains("expired") {
                    Outcome::Expired
                } else if msg.contains("exhaust") {
                    Outcome::Exhausted
                } else if response.status == 0 || response.status >= 500 {
                    Outcome::UnknownOutcome
                } else {
                    Outcome::PermanentFailure
                };
                if game.outcome == Outcome::UnknownOutcome {
                    batch.state = "paused".into();
                }
            }
            if private.key.is_none() {
                game.service_code = response.body["error_code"]
                    .as_i64()
                    .or_else(|| response.body["code"].as_i64());
                game.reason = outcome_reason(game.outcome).into();
                if let Some(explanation) = response.body["error_msg"].as_str() {
                    game.reason = format!("{} {}", game.reason, safe_text(explanation));
                }
                self.store.put_game(&game)?;
                if batch.state == "running" {
                    batch.cursor += 1;
                }
                self.store.put_batch(&batch)?;
                return Ok(());
            }
        }
        let key = private
            .key
            .as_deref()
            .ok_or_else(|| anyhow!("No activatable key"))?;
        if !valid_key(key) {
            game.outcome = Outcome::PermanentFailure;
            game.reason = "This is not a recognized Steam product key.".into();
            self.store.put_game(&game)?;
            batch.cursor += 1;
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        if let Some(previous) = self.duplicate_outcome(&game.id, key)? {
            game.outcome = previous;
            game.reason =
                "The exact same key already has this recorded outcome; no activation was sent."
                    .into();
            self.store.put_game(&game)?;
            if previous == Outcome::UnknownOutcome {
                batch.state = "paused".into();
                batch.message = "An identical key has an uncertain earlier request. Reconcile before continuing.".into();
            } else {
                batch.cursor += 1;
            }
            self.store.put_batch(&batch)?;
            return Ok(());
        }
        let browser = self.connected(Provider::Steam)?;
        let cookies = browser.cookies().await?;
        let session = cookies
            .iter()
            .find(|c| c["name"] == "sessionid")
            .and_then(|c| c["value"].as_str())
            .ok_or_else(|| anyhow!("Steam session needs reconnecting"))?;
        game.outcome = Outcome::InFlight;
        self.store.put_game(&game)?;
        let response = browser
            .fetch(
                "/account/ajaxregisterkey/",
                Some(json!({"product_key":key,"sessionid":session})),
            )
            .await;
        match response {
            Ok(response) if response.status == 200 && response.body["success"].is_number() => {
                let code = if response.body["success"] == 1 {
                    0
                } else {
                    response.body["purchase_result_details"]
                        .as_i64()
                        .or_else(|| {
                            response.body["purchase_receipt_info"]["result_detail"].as_i64()
                        })
                        .unwrap_or(-1)
                };
                game.outcome = steam_outcome(code);
                game.service_code = Some(code);
                game.reason = outcome_reason(game.outcome).into();
                if code == 53 {
                    batch.state = "cooldown".into();
                    batch.next_retry_at = Some(cooldown_deadline(response.retry_after));
                    self.store.setting(
                        "steam-cooldown-until",
                        &batch.next_retry_at.unwrap().to_string(),
                    )?;
                    batch.message="Steam activation limit reached; the approved queue will resume after cooldown.".into();
                } else {
                    batch.cursor += 1;
                    if code == 0
                        && let Some(id) = game.app_id
                    {
                        self.owned.insert(id);
                    }
                }
            }
            Ok(response) if response.status == 429 => {
                game.outcome = Outcome::RateLimited;
                batch.state = "cooldown".into();
                batch.next_retry_at = Some(cooldown_deadline(response.retry_after));
                self.store.setting(
                    "steam-cooldown-until",
                    &batch.next_retry_at.unwrap().to_string(),
                )?;
            }
            Ok(response) if response.status == 401 || response.status == 403 => {
                game.outcome = Outcome::LoginRequired;
                self.connections
                    .get_mut(&Provider::Steam)
                    .unwrap()
                    .connected = false;
                batch.state = "paused".into();
            }
            _ => {
                game.outcome = Outcome::UnknownOutcome;
                game.reason = outcome_reason(game.outcome).into();
                batch.state = "paused".into();
                batch.message =
                    "Steam did not confirm the request. Reconcile ownership before retrying."
                        .into();
            }
        }
        game.updated_at = now();
        self.store.put_game(&game)?;
        self.store.put_batch(&batch)?;
        Ok(())
    }
    fn duplicate_outcome(&self, game_id: &str, key: &str) -> Result<Option<Outcome>> {
        let mut confirmed = None;
        for other in self.store.games()? {
            if other.id == game_id {
                continue;
            }
            if !matches!(
                other.outcome,
                Outcome::Redeemed
                    | Outcome::UsedElsewhere
                    | Outcome::LegacyRedeemed
                    | Outcome::LegacyOwnedOrUsed
                    | Outcome::UnknownOutcome
                    | Outcome::InFlight
            ) {
                continue;
            }
            if self
                .store
                .secret::<PrivateGame>(&format!("game:{}", other.id))?
                .and_then(|p| p.key)
                .as_deref()
                == Some(key)
            {
                if matches!(other.outcome, Outcome::UnknownOutcome | Outcome::InFlight) {
                    return Ok(Some(Outcome::UnknownOutcome));
                }
                confirmed = strongest_key_history(confirmed, other.outcome);
            }
        }
        Ok(confirmed)
    }
    fn reconcile_key_history(&self) -> Result<()> {
        let games = self.store.games()?;
        let mut history = HashMap::new();
        for game in &games {
            if !matches!(
                game.outcome,
                Outcome::Redeemed
                    | Outcome::UsedElsewhere
                    | Outcome::LegacyRedeemed
                    | Outcome::LegacyOwnedOrUsed
                    | Outcome::UnknownOutcome
                    | Outcome::InFlight
            ) {
                continue;
            }
            if let Some(key) = self
                .store
                .secret::<PrivateGame>(&format!("game:{}", game.id))?
                .and_then(|p| p.key)
            {
                let fingerprint = self.id("key-history", &key);
                let outcome = if game.outcome == Outcome::InFlight {
                    Outcome::UnknownOutcome
                } else {
                    game.outcome
                };
                if let Some(result) =
                    strongest_key_history(history.get(&fingerprint).copied(), outcome)
                {
                    history.insert(fingerprint, result);
                }
            }
        }
        for mut game in games.into_iter().filter(|g| g.outcome.eligible()) {
            if let Some(key) = self
                .store
                .secret::<PrivateGame>(&format!("game:{}", game.id))?
                .and_then(|p| p.key)
                && let Some(outcome) = history.get(&self.id("key-history", &key))
            {
                game.outcome = *outcome;
                game.reason = "Exact key matched recorded history; no activation was sent. Legacy history does not establish current account ownership.".into();
                self.store.put_game(&game)?;
            }
        }
        Ok(())
    }
    async fn refresh_owned(&mut self) -> Result<()> {
        let response = self
            .connected(Provider::Steam)?
            .fetch("/dynamicstore/userdata/", None)
            .await?;
        if response.status != 200 || !response.body["rgOwnedApps"].is_array() {
            bail!("Steam ownership lookup failed. No new keys will be activated.")
        }
        self.owned = response.body["rgOwnedApps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| {
                app_id(value).ok_or_else(|| {
                    anyhow!("Steam returned incomplete ownership IDs; activation paused")
                })
            })
            .collect::<Result<_>>()?;
        self.owned_checked = true;
        Ok(())
    }
    pub async fn sync(&mut self) -> Result<()> {
        self.message = "Synchronizing library and Choice availability…".into();
        let orders = {
            let browser = self.connected(Provider::Humble)?;
            let response = browser.fetch("/api/v1/user/order", None).await?;
            if response.status != 200 {
                bail!(
                    "Humble library could not be read; reconnect or complete the browser security check"
                )
            }
            let listing = response
                .body
                .as_array()
                .ok_or_else(|| anyhow!("Humble returned an unexpected order list"))?;
            let paths: Vec<String> = listing.iter().map(|order| {
                let key = order["gamekey"].as_str().filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("Humble returned an incomplete order list; existing history was preserved"))?;
                Ok(format!("/api/v1/order/{}?all_tpkds=true", urlencoding::encode(key)))
            }).collect::<Result<_>>()?;
            let mut orders = Vec::new();
            for chunk in paths.chunks(10) {
                let responses = browser.fetch_many(chunk).await?;
                if responses.len() != chunk.len() {
                    bail!(
                        "Library sync was incomplete. Existing records were preserved; retry synchronization."
                    );
                }
                for (path, mut r) in chunk.iter().zip(responses) {
                    if r.status != 200 || !r.body.is_object() {
                        bail!(
                            "Library sync was incomplete. Existing records were preserved; retry synchronization."
                        )
                    }
                    if !r.body["gamekey"].is_string() {
                        let encoded = path
                            .trim_start_matches("/api/v1/order/")
                            .split('?')
                            .next()
                            .unwrap_or_default();
                        r.body["gamekey"] = json!(urlencoding::decode(encoded)?.into_owned());
                    }
                    orders.push(r.body);
                }
            }
            orders
        };
        for order in &orders {
            self.ingest_order(order)?;
        }
        self.reconcile_key_history()?;
        if self.connections[&Provider::Steam].connected {
            self.refresh_owned().await?;
            for mut game in self.store.games()? {
                if game.outcome.eligible() && game.app_id.is_some_and(|id| self.owned.contains(&id))
                {
                    game.outcome = Outcome::AlreadyOwned;
                    game.reason = outcome_reason(game.outcome).into();
                    self.store.put_game(&game)?;
                }
            }
        }
        self.sync_choices(&orders).await?;
        self.message = format!(
            "Library synchronized: {} recorded items, {} available Choice entries.",
            self.store.games()?.len(),
            self.store.choices()?.len()
        );
        Ok(())
    }
    fn ingest_order(&mut self, order: &Value) -> Result<()> {
        let source = order["product"]["human_name"].as_str();
        let order_id = order["gamekey"].as_str().unwrap_or_default();
        for value in objects(order) {
            let is_steam = value["key_type"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("steam"))
                || value["key_type_human_name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("steam"));
            if !is_steam || value["machine_name"].as_str().is_none() {
                continue;
            }
            let machine = value["machine_name"].as_str().unwrap();
            let index = value.get("keyindex").cloned().unwrap_or(json!(0));
            let own_order = value["gamekey"].as_str().unwrap_or(order_id);
            if own_order.is_empty() {
                bail!("Humble returned an item without its order reference; retry synchronization");
            }
            let id = self.id("game", &format!("{own_order}|{machine}|{index}"));
            let key = value["redeemed_key_val"]
                .as_str()
                .filter(|s| valid_key(s))
                .map(str::to_owned);
            let title = safe_text(value["human_name"].as_str().unwrap_or(machine));
            let previous = self.store.game(&id).ok();
            let outcome = previous
                .as_ref()
                .map(|g| g.outcome)
                .filter(|o| {
                    !matches!(
                        o,
                        Outcome::Unrevealed
                            | Outcome::Ready
                            | Outcome::Exhausted
                            | Outcome::LoginRequired
                    )
                })
                .unwrap_or(if value["is_expired"] == true {
                    Outcome::Expired
                } else if key.is_some() {
                    Outcome::Ready
                } else {
                    Outcome::Unrevealed
                });
            self.store.put_game(&Game {
                id: id.clone(),
                title,
                app_id: value.get("steam_app_id").and_then(app_id),
                source: source
                    .map(safe_text)
                    .or_else(|| previous.as_ref().map(|g| g.source.clone()))
                    .unwrap_or_else(|| "Humble library".into()),
                choice_month: order["product"]["choice_url"]
                    .as_str()
                    .map(safe_text)
                    .or_else(|| previous.as_ref().and_then(|g| g.choice_month.clone())),
                outcome,
                reason: previous
                    .as_ref()
                    .map(|g| g.reason.clone())
                    .unwrap_or_else(|| {
                        if outcome == Outcome::Expired {
                            outcome_reason(outcome).into()
                        } else {
                            String::new()
                        }
                    }),
                service_code: previous.and_then(|g| g.service_code),
                has_key: key.is_some(),
                updated_at: now(),
            })?;
            self.store.put_secret(
                &format!("game:{id}"),
                &PrivateGame {
                    order: own_order.into(),
                    machine: machine.into(),
                    index,
                    key,
                },
            )?;
        }
        Ok(())
    }
    async fn sync_choices(&mut self, orders: &[Value]) -> Result<()> {
        let candidates: Vec<&Value> = orders
            .iter()
            .filter(|o| {
                o["product"]["choice_url"].is_string()
                    && (o["choices_remaining"].as_u64().unwrap_or(0) > 0
                        || o["product"]["is_subs_v3_product"] == true)
            })
            .collect();
        let mut parsed = Vec::new();
        let re = regex::Regex::new(
            r#"(?s)<script[^>]*id=["']webpack-monthly-product-data["'][^>]*>(.*?)</script>"#,
        )?;
        for chunk in candidates.chunks(10) {
            let paths: Vec<String> = chunk
                .iter()
                .map(|o| {
                    format!(
                        "/subscription/{}",
                        urlencoding::encode(
                            o["product"]["choice_url"].as_str().unwrap_or_default()
                        )
                    )
                })
                .collect();
            let responses = self.connected(Provider::Humble)?.fetch_many(&paths).await?;
            if responses.len() != chunk.len() {
                bail!(
                    "Choice sync was incomplete. Existing choices were preserved; retry synchronization."
                );
            }
            for (order, response) in chunk.iter().zip(responses) {
                if response.status != 200 {
                    bail!(
                        "Choice availability could not be fully verified (HTTP {}). Library records were preserved; retry sync.",
                        response.status
                    )
                }
                let text = response.body.as_str().unwrap_or_default();
                let Some(capture) = re.captures(text) else {
                    continue;
                };
                let data: Value = serde_json::from_str(&capture[1]).map_err(|_| {
                    anyhow!("Choice format changed; please report a sanitized diagnostic")
                })?;
                parsed.push(((*order).clone(), data));
            }
        }
        // Keep disabled review records for claims already consumed by an
        // approved batch, so synchronization cannot erase its approval scope.
        let preserved = self.consumed_choice_reviews()?;
        self.store.clear_choices()?;
        for (order, data) in parsed {
            let options = &data["contentChoiceOptions"];
            if options["canRedeemGames"] == false {
                continue;
            }
            let remaining = order["choices_remaining"].as_u64().unwrap_or(0) as usize;
            let unlimited = options["usesChoices"] == false;
            let existing: HashSet<String> = objects(&order["tpkd_dict"])
                .into_iter()
                .filter_map(|v| v["machine_name"].as_str().map(str::to_owned))
                .collect();
            let content = &options["contentChoiceData"];
            let identifier = if unlimited || content["initial"].is_object() {
                "initial"
            } else {
                "initial-classic"
            };
            let game_data = if unlimited {
                &content["game_data"]
            } else {
                &content[identifier]["content_choices"]
            };
            let entries: Vec<&Value> = match game_data {
                Value::Object(m) => m.values().collect(),
                Value::Array(a) => a.iter().collect(),
                _ => Vec::new(),
            };
            let month = order["product"]["choice_url"].as_str().unwrap_or_default();
            let month_id = self.id("month", month);
            for choice in &entries {
                let display = choice["display_item_machine_name"]
                    .as_str()
                    .or_else(|| choice["machine_name"].as_str())
                    .unwrap_or_default();
                let machines: Vec<String> = objects(choice)
                    .into_iter()
                    .filter_map(|v| v["machine_name"].as_str().map(str::to_owned))
                    .collect();
                if existing.contains(display) || machines.iter().any(|m| existing.contains(m)) {
                    continue;
                }
                // Memberships, trials and non-Steam promotions do not enter the activation queue.
                let steam = objects(choice).iter().any(|v| {
                    v["key_type"] == "steam"
                        || v["key_type_human_name"] == "Steam"
                        || v["machine_name"]
                            .as_str()
                            .is_some_and(|s| s.ends_with("_steam"))
                });
                if !steam {
                    continue;
                }
                let id = self.id("choice", &format!("{month}|{display}"));
                let gamekey = choice["tpkds"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v["gamekey"].as_str())
                    .or_else(|| order["gamekey"].as_str())
                    .unwrap_or_default();
                self.store.put_choice(&Choice {
                    id: id.clone(),
                    month_id: month_id.clone(),
                    month: safe_text(order["product"]["human_name"].as_str().unwrap_or(month)),
                    title: safe_text(choice["title"].as_str().unwrap_or(display)),
                    remaining: if unlimited { entries.len() } else { remaining },
                    available: choice["tpkds"].is_array(),
                })?;
                self.store.put_secret(
                    &format!("choice:{id}"),
                    &PrivateChoice {
                        order: gamekey.into(),
                        month: month.into(),
                        identifier: identifier.into(),
                        machine: display.into(),
                        key_machines: machines,
                    },
                )?;
            }
        }
        for choice in preserved {
            self.store.put_choice(&choice)?;
        }
        Ok(())
    }
    fn consumed_choice_reviews(&self) -> Result<Vec<Choice>> {
        let ids: HashSet<String> = self
            .store
            .batches()?
            .into_iter()
            .filter(|b| {
                b.approved
                    && ["approved", "running", "cooldown", "paused"].contains(&b.state.as_str())
            })
            .flat_map(|b| b.choice_ids.into_iter().take(b.choice_cursor))
            .collect();
        Ok(self
            .store
            .choices()?
            .into_iter()
            .filter(|c| ids.contains(&c.id))
            .map(|mut c| {
                c.available = false;
                c
            })
            .collect())
    }
    async fn claim_choice(&mut self, batch: &mut Batch) -> Result<()> {
        let id = &batch.choice_ids[batch.choice_cursor];
        let selected = self
            .store
            .choices()?
            .into_iter()
            .find(|c| &c.id == id)
            .ok_or_else(|| {
                anyhow!("Approved Choice item changed. Synchronize and review a new batch.")
            })?;
        if !selected.available {
            bail!("Choice availability changed; synchronize and review again");
        }
        let private = self
            .store
            .secret::<PrivateChoice>(&format!("choice:{id}"))?
            .ok_or_else(|| anyhow!("Choice details unavailable; synchronize again"))?;
        // Persist intent before the irreversible claim. Never automatically replay an ambiguous claim.
        batch.state = "claiming".into();
        self.store.put_batch(batch)?;
        let response=self.connected(Provider::Humble)?.fetch("/humbler/choosecontent",Some(json!({"gamekey":private.order,"parent_identifier":private.identifier,"chosen_identifiers[]":private.machine,"is_multikey_and_from_choice_modal":"false"}))).await;
        let response = match response {
            Ok(r) => r,
            Err(_) => {
                batch.state = "needs_review".into();
                batch.message =
                    "Choice claim was interrupted. Synchronize and review before retrying.".into();
                return Ok(());
            }
        };
        if response.status != 200
            || !(response.body["success"] == true || response.body["success"] == 1)
        {
            batch.state = "needs_review".into();
            batch.message =
                "Humble did not confirm this Choice claim. Synchronize before continuing.".into();
            return Ok(());
        }
        for mut choice in self.store.choices()? {
            if choice.id == selected.id {
                choice.available = false;
            }
            if choice.month_id == selected.month_id {
                choice.remaining = choice.remaining.saturating_sub(1);
            }
            self.store.put_choice(&choice)?;
        }
        let order = self
            .connected(Provider::Humble)?
            .fetch(
                &format!(
                    "/api/v1/order/{}?all_tpkds=true",
                    urlencoding::encode(&private.order)
                ),
                None,
            )
            .await?;
        if order.status != 200 {
            batch.state = "needs_review".into();
            batch.message = "Choice claimed, but its keys could not yet be synchronized.".into();
            return Ok(());
        }
        let mut order_body = order.body;
        if !order_body["gamekey"].is_string() {
            order_body["gamekey"] = json!(private.order);
        }
        self.ingest_order(&order_body)?;
        let mut matched = Vec::new();
        for game in self.store.games()? {
            if let Some(source) = self
                .store
                .secret::<PrivateGame>(&format!("game:{}", game.id))?
                && source.order == private.order
                && private.key_machines.contains(&source.machine)
                && game.outcome.eligible()
            {
                matched.push(game.id);
            }
        }
        if matched.is_empty() {
            batch.state = "needs_review".into();
            batch.message =
                "Choice claimed. Review its newly synchronized keys before activation.".into();
            return Ok(());
        }
        for id in matched {
            if !batch.game_ids.contains(&id) {
                batch.game_ids.push(id);
            }
        }
        batch.choice_cursor += 1;
        batch.state = "running".into();
        Ok(())
    }
    pub fn import_csv(&mut self, path: &Path) -> Result<usize> {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let outcome = match file_name {
            "redeemed.csv" => Outcome::LegacyRedeemed,
            "already_owned.csv" | "already_owned copy.csv" => Outcome::LegacyOwnedOrUsed,
            "errored.csv" => Outcome::LegacyFailure,
            _ => {
                bail!("Select redeemed.csv, already_owned.csv, or errored.csv from the legacy app")
            }
        };
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_path(path)
            .map_err(|_| anyhow!("Could not open the selected local history file"))?;
        let mut count = 0;
        let mut occurrences = HashMap::<String, usize>::new();
        for record in reader.records() {
            let record = record.map_err(|_| {
                anyhow!("Legacy CSV format is invalid; original file was not changed")
            })?;
            if record.len() < 3 {
                continue;
            }
            let title = safe_text(&record[1]);
            let key = record[2].trim();
            let identity = format!("{file_name}|{}|{title}|{key}", &record[0]);
            let occurrence = occurrences.entry(identity.clone()).or_default();
            *occurrence += 1;
            let id = self.id("legacy-row-v2", &format!("{identity}|{occurrence}"));
            // Replace the early prerelease's collapsed import records, while preserving
            // every source occurrence in the new, idempotent migration format.
            let old_id = self.id("legacy", &format!("{file_name}|{title}|{key}"));
            if self.store.game(&old_id).is_ok() {
                self.store
                    .db
                    .execute("DELETE FROM games WHERE id=?1", [&old_id])?;
                self.store.forget(&format!("game:{old_id}"))?;
            }
            if self.store.game(&id).is_ok() {
                continue;
            }
            self.store.put_game(&Game {
                id: id.clone(),
                title,
                app_id: None,
                source: format!("Legacy import: {file_name}"),
                choice_month: None,
                outcome,
                reason: "Original history did not preserve the exact service reason.".into(),
                service_code: None,
                has_key: valid_key(key),
                updated_at: now(),
            })?;
            if valid_key(key) {
                self.store.put_secret(
                    &format!("game:{id}"),
                    &PrivateGame {
                        order: record[0].into(),
                        key: Some(key.into()),
                        ..Default::default()
                    },
                )?;
            }
            count += 1;
        }
        Ok(count)
    }
    pub fn export(&self, path: &Path, include_keys: bool) -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(|_| {
            anyhow!("Choose a new report filename; existing files are never overwritten")
        })?;
        let mut writer = csv::Writer::from_writer(file);
        writer.write_record(if include_keys {
            vec![
                "title",
                "source",
                "status",
                "reason",
                "updated_at",
                "product_key",
            ]
        } else {
            vec!["title", "source", "status", "reason", "updated_at"]
        })?;
        for game in self.store.games()? {
            let mut row = vec![
                csv_safe(&game.title),
                csv_safe(&game.source),
                game.outcome.label().into(),
                csv_safe(&safe_text(&game.reason)),
                game.updated_at.to_string(),
            ];
            if include_keys {
                let key = self
                    .store
                    .secret::<PrivateGame>(&format!("game:{}", game.id))?
                    .and_then(|p| p.key)
                    .unwrap_or_default();
                row.push(key);
            }
            writer.write_record(row)?;
        }
        writer.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}
fn strongest_key_history(previous: Option<Outcome>, next: Outcome) -> Option<Outcome> {
    fn rank(outcome: Outcome) -> u8 {
        match outcome {
            Outcome::UnknownOutcome | Outcome::InFlight => 5,
            Outcome::Redeemed => 4,
            Outcome::UsedElsewhere => 3,
            Outcome::LegacyRedeemed => 2,
            Outcome::LegacyOwnedOrUsed => 1,
            _ => 0,
        }
    }
    if rank(next) == 0 {
        return previous;
    }
    if previous.is_some_and(|p| rank(p) >= rank(next)) {
        previous
    } else {
        Some(next)
    }
}
fn cooldown_deadline(retry_after: Option<u64>) -> i64 {
    now().saturating_add(i64::try_from(retry_after.unwrap_or(3600).max(60)).unwrap_or(i64::MAX))
}
fn app_id(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| *n > 0)
}
fn objects(value: &Value) -> Vec<&Value> {
    let mut found = Vec::new();
    match value {
        Value::Object(map) => {
            found.push(value);
            for value in map.values() {
                found.extend(objects(value));
            }
        }
        Value::Array(array) => {
            for value in array {
                found.extend(objects(value));
            }
        }
        _ => {}
    }
    found
}
#[cfg(test)]
pub fn legacy_running() -> bool {
    false
}
#[cfg(not(test))]
pub fn legacy_running() -> bool {
    let system = sysinfo::System::new_all();
    system.processes().values().any(|p| {
        p.cmd()
            .iter()
            .any(|s| s.to_string_lossy().ends_with("humblesteamkeysredeemer.py"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(Store::open(dir.path().into(), true).unwrap(), true).unwrap();
        (dir, engine)
    }
    #[test]
    fn reveal_requires_specific_permission() {
        let (_dir, e) = engine();
        assert!(e.plan(vec!["demo-1".into()], vec![], false).is_err());
        assert!(e.plan(vec!["demo-1".into()], vec![], true).is_ok());
    }
    #[test]
    fn claimed_choice_keys_still_require_steam_activation_and_reveal_permission() {
        let (_dir, mut e) = engine();
        e.ingest_order(&json!({
            "gamekey":"synthetic-choice-order",
            "choices_remaining":0,
            "product":{"human_name":"Example Humble Choice","choice_url":"example-month"},
            "tpkd_dict":{"all":[
                {"machine_name":"claimed_fixture_steam","human_name":"Claimed fixture","key_type":"steam","steam_app_id":200,"redeemed":true,"redeemed_key_val":"AAAAA-BBBBB-CCCCC"},
                {"machine_name":"unrevealed_fixture_steam","human_name":"Unrevealed fixture","key_type":"steam","steam_app_id":201,"claimed":true}
            ]}
        })).unwrap();
        let games = e.store.games().unwrap();
        let ready = games.iter().find(|g| g.title == "Claimed fixture").unwrap();
        let unrevealed = games
            .iter()
            .find(|g| g.title == "Unrevealed fixture")
            .unwrap();
        assert_eq!(ready.outcome, Outcome::Ready);
        assert_eq!(unrevealed.outcome, Outcome::Unrevealed);
        assert_eq!(ready.choice_month.as_deref(), Some("example-month"));
        assert!(ready.is_claimed_choice() && unrevealed.is_claimed_choice());
        assert!(
            e.plan(vec![ready.id.clone(), unrevealed.id.clone()], vec![], false)
                .is_err()
        );
        let batch = e
            .plan(vec![ready.id.clone(), unrevealed.id.clone()], vec![], true)
            .unwrap();
        assert!(!batch.approved);
        assert_eq!(batch.state, "review");
        let ready_id = ready.id.clone();
        e.ingest_order(&json!({
            "gamekey":"synthetic-choice-order",
            "tpkd_dict":{"all":[{"machine_name":"claimed_fixture_steam","human_name":"Claimed fixture","key_type":"steam","steam_app_id":200,"redeemed_key_val":"AAAAA-BBBBB-CCCCC"}]}
        })).unwrap();
        let refreshed = e.store.game(&ready_id).unwrap();
        assert_eq!(refreshed.choice_month.as_deref(), Some("example-month"));
        assert_eq!(refreshed.source, "Example Humble Choice");
        assert_eq!(refreshed.outcome, Outcome::Ready);
        assert!(
            !serde_json::to_string(&e.snapshot().unwrap())
                .unwrap()
                .contains("AAAAA-BBBBB-CCCCC")
        );
    }
    #[tokio::test]
    async fn stale_approval_cannot_start() {
        let (_dir, mut e) = engine();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        assert!(
            e.handle(Command::Start {
                batch_id: b.id.clone()
            })
            .await
            .is_err()
        );
        e.store.setting("steam", "changed").unwrap();
        assert!(
            e.handle(Command::Approve {
                batch_id: b.id,
                digest: b.digest
            })
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn approved_demo_batch_completes_without_network() {
        let (_dir, mut e) = engine();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest,
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        e.tick().await.unwrap();
        e.tick().await.unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().state, "complete");
    }
    #[test]
    fn legacy_import_is_idempotent_and_keeps_ambiguous_status() {
        let (dir, mut e) = engine();
        let p = dir.path().join("already_owned.csv");
        std::fs::write(&p, "order,Fixture title,AAAAA-BBBBB-CCCCC\n").unwrap();
        let original = std::fs::read(&p).unwrap();
        assert_eq!(e.import_csv(&p).unwrap(), 1);
        assert_eq!(e.import_csv(&p).unwrap(), 0);
        assert!(
            e.store
                .games()
                .unwrap()
                .iter()
                .any(|g| g.outcome == Outcome::LegacyOwnedOrUsed)
        );
        assert_eq!(std::fs::read(&p).unwrap(), original);
        assert!(
            !serde_json::to_string(&e.snapshot().unwrap())
                .unwrap()
                .contains("AAAAA-BBBBB")
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::fixtures::Fixture;
    #[test]
    fn rotating_cookies_preserve_approval_only_for_the_same_steam_account() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(dir.path().into(), true).unwrap(), true).unwrap();
        e.steam_identity = Some("12345678901234567".into());
        let batch = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        let cookies = vec![
            json!({"name":"steamLoginSecure","value":"12345678901234567%7C%7Crotated-fixture","domain":"store.steampowered.com"}),
        ];
        let binding = e.session_binding(Provider::Steam, &cookies);
        assert_eq!(binding, "demo-binding");
        e.bind_session(Provider::Steam, &binding).unwrap();
        assert!(e.validate_review(&batch).is_ok());
        let other = vec![
            json!({"name":"steamLoginSecure","value":"12345678901234568%7C%7Cfixture","domain":"store.steampowered.com"}),
        ];
        let changed = e.session_binding(Provider::Steam, &other);
        assert_ne!(changed, binding);
        e.bind_session(Provider::Steam, &changed).unwrap();
        assert!(e.validate_review(&batch).is_err());
    }
    #[tokio::test]
    async fn transient_authentication_failure_keeps_saved_state_and_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(dir.path().into(), true).unwrap(), true).unwrap();
        let mut fixture = Fixture::service(
            Provider::Humble,
            json!({"status":503,"body":"temporarily unavailable"}),
        )
        .await;
        e.browsers
            .insert(Provider::Humble, fixture.browser.take().unwrap());
        e.connections.insert(
            Provider::Humble,
            Connection {
                signing_in: true,
                ..Default::default()
            },
        );
        e.demo = false;
        e.store
            .put_secret(
                "session:humble",
                &vec![json!({"name":"fixture","value":"keep-this"})],
            )
            .unwrap();
        e.tick().await.unwrap();
        assert!(e.connections[&Provider::Humble].signing_in);
        assert!(!e.connections[&Provider::Humble].native_login);
        assert!(e.auth_retry_at[&Provider::Humble] >= now() + 59);
        assert!(
            e.store
                .secret::<Vec<Value>>("session:humble")
                .unwrap()
                .is_some()
        );
    }
    async fn live_fixture(response: Value) -> (tempfile::TempDir, Engine, Fixture, Batch) {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(dir.path().into(), true).unwrap(), true).unwrap();
        e.store
            .put_secret(
                "game:demo-0",
                &PrivateGame {
                    key: Some("AAAAA-BBBBB-CCCCC".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut fixture = Fixture::service(Provider::Steam, response).await;
        e.browsers
            .insert(Provider::Steam, fixture.browser.take().unwrap());
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest.clone(),
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        e.demo = false;
        e.owned_checked = true;
        (dir, e, fixture, b)
    }
    #[tokio::test]
    async fn service_results_are_distinct_and_codes_preserved() {
        for (code, expected) in [
            (9, Outcome::AlreadyOwned),
            (15, Outcome::UsedElsewhere),
            (24, Outcome::RetryableFailure),
            (14, Outcome::PermanentFailure),
        ] {
            let (_d, mut e, _f, b) = live_fixture(
                json!({"status":200,"body":{"success":2,"purchase_result_details":code}}),
            )
            .await;
            e.tick().await.unwrap();
            let game = e.store.game("demo-0").unwrap();
            assert_eq!(game.outcome, expected);
            assert_eq!(game.service_code, Some(code));
            assert_eq!(e.store.batch(&b.id).unwrap().cursor, 1);
        }
    }
    #[tokio::test]
    async fn rate_limit_persists_deadline_and_does_not_advance() {
        for response in [
            json!({"status":200,"body":{"success":2,"purchase_result_details":53},"retry_after":7200}),
            json!({"status":429,"body":{},"retry_after":7200}),
        ] {
            let (_d, mut e, _f, b) = live_fixture(response).await;
            e.tick().await.unwrap();
            let before = e.store.batch(&b.id).unwrap();
            assert_eq!(before.state, "cooldown");
            assert_eq!(before.cursor, 0);
            assert!(before.next_retry_at.unwrap() >= now() + 7199);
            e.tick().await.unwrap();
            assert_eq!(
                e.store.batch(&b.id).unwrap().next_retry_at,
                before.next_retry_at
            );
            e.store.recover().unwrap();
            assert_eq!(
                e.store.batch(&b.id).unwrap().next_retry_at,
                before.next_retry_at
            );
        }
    }
    #[tokio::test]
    async fn interrupted_and_partial_responses_pause_without_retry() {
        for response in [
            json!({"status":0,"body":null}),
            json!({"status":200,"body":{}}),
            json!({"status":503,"body":"unavailable"}),
        ] {
            let (_d, mut e, _f, b) = live_fixture(response).await;
            e.tick().await.unwrap();
            assert_eq!(
                e.store.game("demo-0").unwrap().outcome,
                Outcome::UnknownOutcome
            );
            assert_eq!(e.store.batch(&b.id).unwrap().state, "paused");
            e.tick().await.unwrap();
            assert!(
                e.handle(Command::Retry {
                    game_id: "demo-0".into()
                })
                .await
                .is_err()
            );
        }
    }
    #[tokio::test]
    async fn expired_sessions_pause_and_require_human_login() {
        let (_d, mut e, _f, b) = live_fixture(json!({"status":401,"body":null})).await;
        e.tick().await.unwrap();
        assert_eq!(
            e.store.game("demo-0").unwrap().outcome,
            Outcome::LoginRequired
        );
        assert!(!e.snapshot().unwrap().steam.connected);
        assert_eq!(e.store.batch(&b.id).unwrap().state, "paused");
    }
    #[tokio::test]
    async fn exact_ownership_skips_activation_without_browser() {
        let (_d, mut e, _f, b) = live_fixture(json!({"status":0,"body":null})).await;
        e.browsers.clear();
        e.owned.insert(100);
        e.tick().await.unwrap();
        assert_eq!(
            e.store.game("demo-0").unwrap().outcome,
            Outcome::AlreadyOwned
        );
        assert_eq!(e.store.batch(&b.id).unwrap().cursor, 1);
    }
    #[tokio::test]
    async fn cancelled_batch_cannot_be_reapproved_or_resumed() {
        let (_d, mut e, _f, b) = live_fixture(Value::Null).await;
        e.handle(Command::Cancel {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        assert!(
            e.handle(Command::Approve {
                batch_id: b.id.clone(),
                digest: b.digest
            })
            .await
            .is_err()
        );
        assert!(e.handle(Command::Resume { batch_id: b.id }).await.is_err());
    }
    #[tokio::test]
    async fn duplicate_keys_inherit_recorded_outcome_without_activation() {
        let (_d, mut e, _f, b) = live_fixture(Value::Null).await;
        e.store
            .put_secret(
                "game:demo-3",
                &PrivateGame {
                    key: Some("AAAAA-BBBBB-CCCCC".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        e.browsers.clear();
        e.tick().await.unwrap();
        assert_eq!(
            e.store.game("demo-0").unwrap().outcome,
            Outcome::UsedElsewhere
        );
        assert_eq!(e.store.batch(&b.id).unwrap().cursor, 1);
    }
    #[tokio::test]
    async fn duplicate_history_does_not_invent_current_account_ownership() {
        for recorded in [
            Outcome::LegacyRedeemed,
            Outcome::LegacyOwnedOrUsed,
            Outcome::Redeemed,
        ] {
            let (_d, mut e, _f, b) = live_fixture(Value::Null).await;
            let mut history = e.store.game("demo-3").unwrap();
            history.outcome = recorded;
            e.store.put_game(&history).unwrap();
            e.store
                .put_secret(
                    "game:demo-3",
                    &PrivateGame {
                        key: Some("AAAAA-BBBBB-CCCCC".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            e.browsers.clear();
            e.tick().await.unwrap();
            assert_eq!(e.store.game("demo-0").unwrap().outcome, recorded);
            assert_eq!(e.store.batch(&b.id).unwrap().cursor, 1);
        }
    }
    #[tokio::test]
    async fn owned_unused_key_is_rechecked_after_session_change() {
        let (_d, mut e, _f, _b) = live_fixture(Value::Null).await;
        e.store
            .put_secret(
                "game:demo-2",
                &PrivateGame {
                    key: Some("AAAAA-BBBBB-CCCCC".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            e.duplicate_outcome("demo-0", "AAAAA-BBBBB-CCCCC").unwrap(),
            None
        );
        e.bind_session(Provider::Steam, "new-session").unwrap();
        assert_eq!(e.store.game("demo-2").unwrap().outcome, Outcome::Ready);
    }
    #[tokio::test]
    async fn sync_reconciles_exact_keys_and_preserves_legacy_ambiguity() {
        let (_d, mut e, _f, _b) = live_fixture(Value::Null).await;
        let mut history = e.store.game("demo-3").unwrap();
        history.outcome = Outcome::LegacyOwnedOrUsed;
        e.store.put_game(&history).unwrap();
        e.store
            .put_secret(
                "game:demo-3",
                &PrivateGame {
                    key: Some("AAAAA-BBBBB-CCCCC".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        e.reconcile_key_history().unwrap();
        assert_eq!(
            e.store.game("demo-0").unwrap().outcome,
            Outcome::LegacyOwnedOrUsed
        );
        assert_eq!(
            e.store.game("demo-3").unwrap().outcome,
            Outcome::LegacyOwnedOrUsed
        );
    }
    #[test]
    fn export_does_not_overwrite_and_defaults_to_no_keys() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let p = d.path().join("export.csv");
        e.export(&p, false).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("product_key"));
        assert!(e.export(&p, true).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[cfg(test)]
mod reveal_recovery_tests {
    use super::*;
    use crate::fixtures::Fixture;
    #[tokio::test]
    async fn expired_and_exhausted_are_preserved_with_sanitized_service_reason() {
        for (message, outcome) in [
            ("Key expired", Outcome::Expired),
            ("Keys exhausted", Outcome::Exhausted),
        ] {
            let d = tempfile::tempdir().unwrap();
            let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
            e.store
                .put_secret(
                    "game:demo-1",
                    &PrivateGame {
                        order: "synthetic-order".into(),
                        machine: "fixture_steam".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut f = Fixture::service(
                Provider::Humble,
                json!({"status":200,"body":{"error_msg":message,"error_code":7}}),
            )
            .await;
            e.browsers
                .insert(Provider::Humble, f.browser.take().unwrap());
            let b = e.plan(vec!["demo-1".into()], vec![], true).unwrap();
            e.handle(Command::Approve {
                batch_id: b.id.clone(),
                digest: b.digest,
            })
            .await
            .unwrap();
            e.handle(Command::Start {
                batch_id: b.id.clone(),
            })
            .await
            .unwrap();
            e.demo = false;
            e.owned_checked = true;
            e.tick().await.unwrap();
            let g = e.store.game("demo-1").unwrap();
            assert_eq!(g.outcome, outcome);
            assert_eq!(g.service_code, Some(7));
            assert!(g.reason.contains(message));
            assert!(!g.has_key);
        }
    }
    #[tokio::test]
    async fn ambiguous_choice_is_not_replayed_after_restart() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        e.store
            .put_secret(
                "choice:demo-choice",
                &PrivateChoice {
                    order: "synthetic-order".into(),
                    month: "fixture".into(),
                    identifier: "initial".into(),
                    machine: "fixture_steam".into(),
                    key_machines: vec!["fixture_steam".into()],
                },
            )
            .unwrap();
        let mut f = Fixture::service(Provider::Humble, json!({"status":0,"body":null})).await;
        e.browsers
            .insert(Provider::Humble, f.browser.take().unwrap());
        let b = e.plan(vec![], vec!["demo-choice".into()], true).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest,
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        e.demo = false;
        e.tick().await.unwrap();
        e.store.recover().unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().state, "needs_review");
        assert!(e.handle(Command::Resume { batch_id: b.id }).await.is_err());
    }
    #[tokio::test]
    async fn partial_owned_ids_do_not_allow_activations() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let mut f = Fixture::service(
            Provider::Steam,
            json!({"status":200,"body":{"rgOwnedApps":[100,"not-an-id"]}}),
        )
        .await;
        e.browsers
            .insert(Provider::Steam, f.browser.take().unwrap());
        assert!(e.refresh_owned().await.is_err());
        assert!(!e.owned_checked);
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    #[tokio::test]
    async fn changed_reviewed_title_or_app_requires_new_approval() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        let mut g = e.store.game("demo-0").unwrap();
        g.app_id = Some(999);
        e.store.put_game(&g).unwrap();
        assert!(
            e.handle(Command::Approve {
                batch_id: b.id,
                digest: b.digest
            })
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn running_batch_cannot_activate_changed_review_scope() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest.clone(),
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        let mut game = e.store.game("demo-0").unwrap();
        game.title = "Different fixture after synchronization".into();
        e.store.put_game(&game).unwrap();
        e.tick().await.unwrap();
        let current = e.store.batch(&b.id).unwrap();
        assert_eq!(current.state, "paused");
        assert!(!current.approved);
        assert_eq!(current.cursor, 0);
        assert_eq!(e.store.game("demo-0").unwrap().outcome, Outcome::Ready);
    }
    #[tokio::test]
    async fn consumed_choice_scope_survives_sync_without_becoming_available() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let mut b = e.plan(vec![], vec!["demo-choice".into()], true).unwrap();
        b.approved = true;
        b.state = "running".into();
        b.choice_cursor = 1;
        e.store.put_batch(&b).unwrap();
        let preserved = e.consumed_choice_reviews().unwrap();
        e.store.clear_choices().unwrap();
        for choice in preserved {
            e.store.put_choice(&choice).unwrap();
        }
        assert!(!e.store.choices().unwrap()[0].available);
        e.validate_review(&b).unwrap();
        e.tick().await.unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().state, "complete");
    }
}

#[cfg(test)]
mod migration_occurrence_tests {
    use super::*;
    #[test]
    fn duplicate_history_rows_and_distinct_orders_are_preserved_idempotently() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let p = d.path().join("errored.csv");
        let original=b"fixture-order,Same fixture,False\nfixture-order,Same fixture,False\nother-order,Same fixture,False\n";
        std::fs::write(&p, original).unwrap();
        assert_eq!(e.import_csv(&p).unwrap(), 3);
        assert_eq!(e.import_csv(&p).unwrap(), 0);
        assert_eq!(
            e.store
                .games()
                .unwrap()
                .iter()
                .filter(|g| g.outcome == Outcome::LegacyFailure)
                .count(),
            3
        );
        assert_eq!(std::fs::read(&p).unwrap(), original);
    }
}

#[cfg(test)]
mod cooldown_scope_tests {
    use super::*;
    #[tokio::test]
    async fn a_new_batch_cannot_bypass_a_persisted_service_cooldown() {
        let d = tempfile::tempdir().unwrap();
        let mut e = Engine::new(Store::open(d.path().into(), true).unwrap(), true).unwrap();
        let deadline = now() + 3600;
        e.store
            .setting("steam-cooldown-until", &deadline.to_string())
            .unwrap();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest,
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().state, "cooldown");
        e.tick().await.unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().cursor, 0);
        e.handle(Command::Cancel { batch_id: b.id }).await.unwrap();
        let b = e.plan(vec!["demo-0".into()], vec![], false).unwrap();
        e.handle(Command::Approve {
            batch_id: b.id.clone(),
            digest: b.digest,
        })
        .await
        .unwrap();
        e.handle(Command::Start {
            batch_id: b.id.clone(),
        })
        .await
        .unwrap();
        assert_eq!(e.store.batch(&b.id).unwrap().next_retry_at, Some(deadline));
        e.store.recover().unwrap();
        assert_eq!(e.snapshot().unwrap().steam_retry_at, Some(deadline));
    }
}
