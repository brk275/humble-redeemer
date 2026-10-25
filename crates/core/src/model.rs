use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Humble,
    Steam,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Humble => "humble",
            Self::Steam => "steam",
        }
    }
    pub fn origin(self) -> &'static str {
        match self {
            Self::Humble => "https://www.humblebundle.com",
            Self::Steam => "https://store.steampowered.com",
        }
    }
    pub fn login(self) -> &'static str {
        match self {
            Self::Humble => "https://www.humblebundle.com/login",
            Self::Steam => "https://store.steampowered.com/login/?redir=account%2Fregisterkey",
        }
    }
    /// Only the service's store and its explicit renewal host can hold sessions.
    pub fn accepts_cookie_domain(self, domain: &str) -> bool {
        let domain = domain.trim_start_matches('.');
        match self {
            Self::Humble => matches!(domain, "humblebundle.com" | "www.humblebundle.com"),
            Self::Steam => matches!(
                domain,
                "steampowered.com" | "store.steampowered.com" | "login.steampowered.com"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserKind {
    Firefox,
    Chrome,
}
impl BrowserKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Firefox => "firefox",
            Self::Chrome => "chrome",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    Ready,
    Unrevealed,
    Queued,
    InFlight,
    Redeemed,
    AlreadyOwned,
    UsedElsewhere,
    Expired,
    Exhausted,
    RateLimited,
    LoginRequired,
    RetryableFailure,
    PermanentFailure,
    UnknownOutcome,
    LegacyRedeemed,
    LegacyOwnedOrUsed,
    LegacyFailure,
}
impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Unrevealed => "Unrevealed",
            Self::Queued => "Queued",
            Self::InFlight => "In progress",
            Self::Redeemed => "Redeemed",
            Self::AlreadyOwned => "Already owned",
            Self::UsedElsewhere => "Activated elsewhere",
            Self::Expired => "Expired",
            Self::Exhausted => "Temporarily exhausted",
            Self::RateLimited => "Activation cooldown",
            Self::LoginRequired => "Sign-in required",
            Self::RetryableFailure => "Retry available",
            Self::PermanentFailure => "Failed",
            Self::UnknownOutcome => "Needs reconciliation",
            Self::LegacyRedeemed => "Legacy redeemed",
            Self::LegacyOwnedOrUsed => "Legacy owned/used",
            Self::LegacyFailure => "Legacy failure",
        }
    }
    pub fn eligible(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::Unrevealed
                | Self::Exhausted
                | Self::RetryableFailure
                | Self::LoginRequired
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub app_id: Option<u32>,
    pub source: String,
    #[serde(default)]
    pub choice_month: Option<String>,
    pub outcome: Outcome,
    pub reason: String,
    pub service_code: Option<i64>,
    pub has_key: bool,
    pub updated_at: i64,
}
impl Game {
    pub fn is_claimed_choice(&self) -> bool {
        // Older local records lack provenance. Their official source label is
        // a display/filter fallback only; it never establishes Steam ownership.
        self.choice_month.is_some() || self.source.ends_with(" Humble Choice")
    }
    pub fn matches_library_query(
        &self,
        search: &str,
        eligible_only: bool,
        choice_only: bool,
    ) -> bool {
        let query = search.to_lowercase();
        (!eligible_only || self.outcome.eligible())
            && (!choice_only || self.is_claimed_choice())
            && (self.title.to_lowercase().contains(&query)
                || self.source.to_lowercase().contains(&query))
    }
}

// Internal data is never included in snapshots, logs, CLI output, or MCP schemas.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct PrivateGame {
    pub order: String,
    pub machine: String,
    pub index: Value,
    pub key: Option<String>,
}
impl std::fmt::Debug for PrivateGame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[private game data]")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub month_id: String,
    pub month: String,
    pub title: String,
    pub remaining: usize,
    pub available: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PrivateChoice {
    pub order: String,
    pub month: String,
    pub identifier: String,
    pub machine: String,
    pub key_machines: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Connection {
    pub connected: bool,
    pub signing_in: bool,
    #[serde(default)]
    pub native_login: bool,
    #[serde(default)]
    pub session_renewal_available: bool,
    pub message: String,
    pub browser: Option<BrowserKind>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Batch {
    pub id: String,
    pub digest: String,
    pub game_ids: Vec<String>,
    #[serde(default)]
    pub reviewed_game_ids: Vec<String>,
    pub choice_ids: Vec<String>,
    pub allow_reveal: bool,
    pub approved: bool,
    /// Computed by the worker for snapshots; never grants permission to run.
    #[serde(default)]
    pub review_required: bool,
    pub state: String,
    pub cursor: usize,
    pub choice_cursor: usize,
    pub next_retry_at: Option<i64>,
    pub binding: String,
    pub message: String,
}
impl Batch {
    pub fn can_resume(&self) -> bool {
        self.approved
            && !self.review_required
            && ["approved", "paused"].contains(&self.state.as_str())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub psn: crate::psn::State,
    pub version: String,
    pub demo: bool,
    pub persistent_vault: bool,
    #[serde(default)]
    pub steam_retry_at: Option<i64>,
    pub humble: Connection,
    pub steam: Connection,
    pub games: Vec<Game>,
    pub choices: Vec<Choice>,
    pub batches: Vec<Batch>,
    pub message: String,
    pub browsers: Vec<BrowserKind>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    Psn {
        command: crate::psn::Command,
    },
    Snapshot,
    Connect {
        provider: Provider,
        browser: Option<BrowserKind>,
    },
    Disconnect {
        provider: Provider,
    },
    // Internal GUI-to-worker operation. Deliberately absent from CLI/MCP tools.
    AttachNativeSession {
        provider: Provider,
        cookies: Vec<Value>,
        user_agent: String,
    },
    Sync,
    Ownership {
        app_ids: Vec<u32>,
    },
    Plan {
        game_ids: Vec<String>,
        choice_ids: Vec<String>,
        allow_reveal: bool,
    },
    Approve {
        batch_id: String,
        digest: String,
    },
    Start {
        batch_id: String,
    },
    Pause {
        batch_id: String,
    },
    Resume {
        batch_id: String,
    },
    Cancel {
        batch_id: String,
    },
    Retry {
        game_id: String,
    },
    Import {
        path: String,
    },
    Export {
        path: String,
        include_keys: bool,
    },
    RevealKey {
        game_id: String,
    },
    InstallDriver {
        browser: BrowserKind,
    },
    Shutdown {
        pause: bool,
    },
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn steam_outcome(code: i64) -> Outcome {
    match code {
        0 => Outcome::Redeemed,
        9 => Outcome::AlreadyOwned,
        15 => Outcome::UsedElsewhere,
        53 => Outcome::RateLimited,
        24 => Outcome::RetryableFailure,
        _ => Outcome::PermanentFailure,
    }
}
pub fn outcome_reason(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Redeemed => "Steam confirmed activation.",
        Outcome::AlreadyOwned => "The connected Steam account already owns this product.",
        Outcome::UsedElsewhere => "Steam reports that this key was previously activated elsewhere.",
        Outcome::RateLimited => {
            "Steam requested a cooldown. The queue will wait before trying again."
        }
        Outcome::Expired => "Humble reports that the key has expired.",
        Outcome::Exhausted => "Humble has temporarily exhausted its supply of keys.",
        Outcome::UnknownOutcome => {
            "The request was interrupted. Check ownership before attempting this key again."
        }
        Outcome::RetryableFailure => {
            "The product could not be activated yet; review its prerequisites before retrying."
        }
        _ => "The service did not confirm activation. Review this item before retrying.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguish_ownership_and_used_keys() {
        assert_eq!(steam_outcome(9), Outcome::AlreadyOwned);
        assert_eq!(steam_outcome(15), Outcome::UsedElsewhere);
        assert_ne!(steam_outcome(9), steam_outcome(15));
    }
    #[test]
    fn expired_is_not_automatically_retried() {
        assert!(!Outcome::Expired.eligible());
        assert!(!Outcome::UnknownOutcome.eligible());
        assert!(Outcome::Exhausted.eligible());
    }
    #[test]
    fn older_choice_records_are_queryable_without_changing_their_status() {
        let game: Game = serde_json::from_value(serde_json::json!({
            "id":"fixture","title":"Example game","app_id":null,
            "source":"Example month Humble Choice","outcome":"unrevealed",
            "reason":"","service_code":null,"has_key":false,"updated_at":0
        }))
        .unwrap();
        assert!(game.choice_month.is_none());
        assert!(game.matches_library_query("EXAMPLE MONTH", true, true));
        assert_eq!(game.outcome, Outcome::Unrevealed);
        let mut legacy = game.clone();
        legacy.source = "Legacy import: already_owned.csv".into();
        legacy.outcome = Outcome::LegacyOwnedOrUsed;
        assert!(!legacy.is_claimed_choice());
        assert!(!legacy.matches_library_query("", true, false));
    }
}
