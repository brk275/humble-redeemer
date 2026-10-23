#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod login;
use eframe::egui::{self, Color32, RichText, Vec2};
use humble_redeemer_core::{ipc, model::*, privacy::safe_text};
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::mpsc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const ACCENT: Color32 = Color32::from_rgb(82, 218, 185);
const MUTED: Color32 = Color32::from_rgb(155, 170, 191);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Library,
    Choice,
    Review,
    Queue,
    Results,
    Settings,
}
impl Page {
    fn name(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Library => "Library",
            Self::Choice => "Humble Choice",
            Self::Review => "Review batch",
            Self::Queue => "Active queue",
            Self::Results => "Results",
            Self::Settings => "Settings",
        }
    }
}
struct App {
    demo: bool,
    page: Page,
    snapshot: Option<Snapshot>,
    selected: HashSet<String>,
    choice_selected: HashSet<String>,
    search: String,
    only_eligible: bool,
    only_claimed_choice: bool,
    allow_reveal: bool,
    review: Option<Batch>,
    rx: Option<mpsc::Receiver<Result<Value, String>>>,
    last_poll: Instant,
    error: String,
    busy: String,
    browser: Option<BrowserKind>,
    show_key: Option<(String, Zeroizing<String>, Instant)>,
    export_keys: bool,
    smoke: bool,
    frames: usize,
    smoke_started: Instant,
    quit_after_response: bool,
    login: Option<login::Login>,
    login_requested: Option<Provider>,
    login_rect: egui::Rect,
    login_fixture: bool,
    dismissed_logins: HashSet<Provider>,
    #[cfg(target_os = "macos")]
    native_menu: Option<muda::Menu>,
}
impl App {
    fn new(demo: bool, smoke: bool) -> Self {
        Self {
            demo,
            page: Page::Overview,
            snapshot: None,
            selected: HashSet::new(),
            choice_selected: HashSet::new(),
            search: String::new(),
            only_eligible: false,
            only_claimed_choice: false,
            allow_reveal: false,
            review: None,
            rx: None,
            last_poll: Instant::now() - Duration::from_secs(10),
            error: String::new(),
            busy: String::new(),
            browser: None,
            show_key: None,
            export_keys: false,
            smoke,
            frames: 0,
            smoke_started: Instant::now(),
            quit_after_response: false,
            login: None,
            login_requested: None,
            login_rect: egui::Rect::NOTHING,
            login_fixture: false,
            dismissed_logins: HashSet::new(),
            #[cfg(target_os = "macos")]
            native_menu: None,
        }
    }
    fn dispatch(&mut self, command: Command, label: &str) {
        if self.rx.is_some() {
            return;
        }
        if !matches!(command, Command::Snapshot) {
            self.error.clear();
        }
        self.busy = label.into();
        let (tx, rx) = mpsc::channel();
        let demo = self.demo;
        std::thread::spawn(move || {
            let result =
                ipc::blocking_request(demo, command).map_err(|e| safe_text(&e.to_string()));
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
    }
    fn approve_start(&mut self, batch: Batch) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let demo = self.demo;
        self.busy = "Starting approved batch…".into();
        std::thread::spawn(move || {
            let result = (|| {
                if !batch.approved {
                    ipc::blocking_request(
                        demo,
                        Command::Approve {
                            batch_id: batch.id.clone(),
                            digest: batch.digest,
                        },
                    )?;
                }
                ipc::blocking_request(demo, Command::Start { batch_id: batch.id })
            })()
            .map_err(|e: anyhow::Error| safe_text(&e.to_string()));
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
    }
    fn receive(&mut self) {
        let result = self.rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.rx = None;
            self.busy.clear();
            self.last_poll = Instant::now();
            match result {
                Ok(value) if value.get("digest").is_some() => {
                    match serde_json::from_value::<Batch>(value) {
                        Ok(batch) => {
                            self.review = Some(batch);
                            self.page = Page::Review;
                        }
                        Err(_) => self.error = "Invalid batch response".into(),
                    }
                }
                Ok(value) if value.get("key").is_some() => {
                    if let Some(key) = value["key"].as_str() {
                        self.show_key = Some((
                            "Private product key".into(),
                            Zeroizing::new(key.into()),
                            Instant::now(),
                        ));
                    }
                }
                Ok(value) => match serde_json::from_value::<Snapshot>(value) {
                    Ok(snapshot) => {
                        if let Some(review) = &mut self.review
                            && let Some(current) =
                                snapshot.batches.iter().find(|b| b.id == review.id)
                        {
                            *review = current.clone();
                        } else if self.review.is_none() {
                            self.review = snapshot
                                .batches
                                .iter()
                                .rev()
                                .find(|b| b.state == "review")
                                .cloned();
                        }
                        self.snapshot = Some(snapshot);
                    }
                    Err(_) => self.error = "Invalid worker response".into(),
                },
                Err(error) => {
                    if let Some(login) = &mut self.login
                        && login.submitted
                    {
                        login.handoff_failed(&error);
                    }
                    self.error = error;
                }
            }
        }
    }
    fn refresh(&mut self) {
        self.dispatch(
            Command::Snapshot,
            if self.snapshot.is_none() {
                "Starting worker… respond to any system keychain prompt"
            } else {
                "Refreshing…"
            },
        )
    }
    fn plan(&mut self) {
        self.dispatch(
            Command::Plan {
                game_ids: self.selected.iter().cloned().collect(),
                choice_ids: self.choice_selected.iter().cloned().collect(),
                allow_reveal: self.allow_reveal,
            },
            "Preparing batch review…",
        )
    }
    fn render(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 28, 38))
                    .inner_margin(18.),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("HR").size(24.).strong().color(ACCENT));
                    ui.add_space(12.);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Humble Redeemer").size(21.).strong());
                        ui.label(
                            RichText::new("Your keys. Your computer. Your control.")
                                .small()
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.demo {
                                "DEMO · synthetic data"
                            } else {
                                "LOCAL · private by design"
                            })
                            .color(ACCENT),
                        );
                    });
                });
            });
        egui::SidePanel::left("navigation")
            .exact_width(190.)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 25, 34))
                    .inner_margin(18.),
            )
            .show(ctx, |ui| {
                ui.add_space(15.);
                ui.label(RichText::new("WORKSPACE").small().color(MUTED));
                ui.add_space(12.);
                for page in [
                    Page::Overview,
                    Page::Library,
                    Page::Choice,
                    Page::Review,
                    Page::Queue,
                    Page::Results,
                    Page::Settings,
                ] {
                    if ui
                        .add_sized(
                            [150., 35.],
                            egui::Button::new(page.name()).selected(self.page == page),
                        )
                        .clicked()
                    {
                        self.page = page;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        RichText::new(format!("v{} · unofficial", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(MUTED),
                    );
                    ui.label(RichText::new("No telemetry").small().color(MUTED));
                });
            });
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 28, 38))
                    .inner_margin(12.),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.rx.is_some() {
                        ui.spinner();
                        ui.label(&self.busy);
                    } else if let Some(snapshot) = &self.snapshot {
                        ui.label(RichText::new(&snapshot.message).small().color(MUTED));
                    } else {
                        ui.label("Starting private worker…");
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(14, 19, 27))
                    .inner_margin(28.),
            )
            .show(ctx, |ui| {
                if !self.error.is_empty() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(66, 31, 38))
                        .corner_radius(8.)
                        .inner_margin(12.)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&self.error).color(Color32::from_rgb(255, 185, 185)),
                            );
                        });
                    ui.add_space(12.);
                }
                ui.heading(RichText::new(self.page.name()).size(29.));
                ui.add_space(8.);
                if self.login.is_some() {
                    self.login_panel(ui);
                    return;
                }
                match self.page {
                    Page::Overview => self.overview(ui),
                    Page::Library => self.library(ui, false),
                    Page::Results => self.library(ui, true),
                    Page::Choice => self.choices(ui),
                    Page::Review => self.review(ui),
                    Page::Queue => self.queue(ui),
                    Page::Settings => self.settings(ui),
                }
            });
        if self
            .show_key
            .as_ref()
            .is_some_and(|(_, _, time)| time.elapsed() > Duration::from_secs(20))
        {
            self.show_key = None;
        }
        if let Some((title, key, _)) = &self.show_key {
            let title = title.clone();
            let key = key.to_string();
            let mut close = false;
            egui::Window::new(title).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("Visible only because you explicitly requested it. Hidden again after 20 seconds.");
                ui.add_space(12.);
                ui.monospace(&key);
                ui.horizontal(|ui| {
                    if ui.button("Copy to local clipboard").clicked() { ui.ctx().copy_text(key.clone()); }
                    if ui.button("Hide now").clicked() { close=true; }
                });
            });
            if close {
                self.show_key = None;
            }
        }
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Connect once. Review a batch. Let the queue handle the rest.")
                .color(MUTED),
        );
        ui.add_space(22.);
        if let Some(snapshot) = &self.snapshot {
            let total = snapshot.games.len();
            let ready = snapshot
                .games
                .iter()
                .filter(|g| g.outcome.eligible())
                .count();
            let redeemed = snapshot
                .games
                .iter()
                .filter(|g| g.outcome == Outcome::Redeemed)
                .count();
            let attention = snapshot
                .games
                .iter()
                .filter(|g| {
                    matches!(
                        g.outcome,
                        Outcome::UnknownOutcome
                            | Outcome::Expired
                            | Outcome::Exhausted
                            | Outcome::UsedElsewhere
                            | Outcome::PermanentFailure
                    )
                })
                .count();
            ui.columns(4, |columns| {
                for (column, (label, count)) in columns.iter_mut().zip([
                    ("Library items", total),
                    ("Ready to review", ready),
                    ("Redeemed", redeemed),
                    ("Need attention", attention),
                ]) {
                    card(column, |ui| {
                        ui.label(
                            RichText::new(count.to_string())
                                .size(34.)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.label(RichText::new(label).color(MUTED));
                    });
                }
            });
        }
        if let Some(deadline) = self.snapshot.as_ref().and_then(|s| s.steam_retry_at) {
            ui.add_space(12.);
            ui.label(
                RichText::new(format!(
                    "Steam cooldown: {} minutes remaining. New batches honor this same deadline.",
                    ((deadline - now()).max(0) + 59) / 60
                ))
                .color(Color32::from_rgb(242, 193, 114)),
            );
        }
        ui.add_space(24.);
        ui.label(RichText::new("ACCOUNT CONNECTIONS").small().color(MUTED));
        ui.add_space(10.);
        for provider in [Provider::Humble, Provider::Steam] {
            let connection = self
                .snapshot
                .as_ref()
                .map(|s| {
                    if provider == Provider::Humble {
                        s.humble.clone()
                    } else {
                        s.steam.clone()
                    }
                })
                .unwrap_or_default();
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if provider == Provider::Humble {
                                "Humble Bundle"
                            } else {
                                "Steam"
                            })
                            .size(18.)
                            .strong(),
                        );
                        ui.label(
                            RichText::new(if connection.message.is_empty() {
                                "Sign in directly on the official website."
                            } else {
                                &connection.message
                            })
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if connection.connected {
                            ui.label(RichText::new("Connected").color(ACCENT));
                        } else if ui
                            .add_enabled(
                                self.rx.is_none(),
                                egui::Button::new(if connection.signing_in {
                                    "Reconnect"
                                } else {
                                    "Connect account"
                                }),
                            )
                            .clicked()
                        {
                            self.dismissed_logins.remove(&provider);
                            self.login_requested = Some(provider);
                            self.dispatch(
                                Command::Connect {
                                    provider,
                                    browser: None,
                                },
                                "Opening in-app sign-in…",
                            );
                        }
                    });
                });
            });
            ui.add_space(10.);
        }
        ui.add_space(16.);
        ui.horizontal(|ui| {
            if primary(ui, "Synchronize library", self.rx.is_none()) {
                self.dispatch(Command::Sync, "Synchronizing your library…");
            }
            if ui.button("Choose games to redeem").clicked() {
                self.page = Page::Library;
            }
        });
        ui.add_space(24.);
        card(ui, |ui| {
            ui.label(RichText::new("Every outcome has a reason.").strong());
            ui.label(RichText::new("Owned, previously activated, expired and temporarily exhausted keys are recorded separately. Uncertain requests pause for review instead of being blindly retried.").color(MUTED));
        });
    }
    fn library(&mut self, ui: &mut egui::Ui, results: bool) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search titles or source…")
                    .id(egui::Id::new("library-search"))
                    .desired_width(300.),
            );
            ui.checkbox(&mut self.only_eligible, "Eligible only");
            if results && ui.button("Export redacted report").clicked() {
                self.export_dialog(false);
            }
        });
        ui.checkbox(
            &mut self.only_claimed_choice,
            "Claimed Humble Choice keys only",
        );
        if !results {
            ui.label(RichText::new("Claimed on Humble means a key is in your Humble library. Ready and Unrevealed still need a reviewed Steam activation batch.").small().color(MUTED));
        }
        ui.add_space(12.);
        let games = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.games
                    .iter()
                    .filter(|g| {
                        g.matches_library_query(
                            &self.search,
                            self.only_eligible,
                            self.only_claimed_choice,
                        ) && (!results
                            || !matches!(g.outcome, Outcome::Ready | Outcome::Unrevealed))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !results {
            ui.horizontal(|ui| {
                if ui.button("Select all eligible shown").clicked() {
                    for game in &games {
                        if game.outcome.eligible() {
                            self.selected.insert(game.id.clone());
                        }
                    }
                }
                if ui.button("Clear selection").clicked() {
                    self.selected.clear();
                }
                ui.label(RichText::new(format!("{} selected", self.selected.len())).color(ACCENT));
                if primary(
                    ui,
                    "Review selected",
                    !self.selected.is_empty() && self.rx.is_none(),
                ) {
                    self.plan();
                }
            });
            ui.checkbox(
                &mut self.allow_reveal,
                "Allow this batch to reveal unrevealed keys (removes gift-link availability)",
            );
        }
        ui.separator();
        if games.is_empty() {
            ui.add_space(30.);
            ui.label(RichText::new("No matching items yet. Connect your accounts and synchronize, or import local history in Settings.").color(MUTED));
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 58., games.len(), |ui, range| {
                for index in range {
                    let game = &games[index];
                    ui.push_id(&game.id, |ui| {
                        ui.horizontal(|ui| {
                            if !results {
                                let mut checked = self.selected.contains(&game.id);
                                if ui
                                    .add_enabled(
                                        game.outcome.eligible(),
                                        egui::Checkbox::without_text(&mut checked),
                                    )
                                    .on_hover_text(format!("Select {}", game.title))
                                    .changed()
                                {
                                    if checked {
                                        self.selected.insert(game.id.clone());
                                    } else {
                                        self.selected.remove(&game.id);
                                    }
                                }
                            }
                            ui.allocate_ui_with_layout(
                                Vec2::new((ui.available_width() - 290.).max(240.), 50.),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    ui.label(RichText::new(&game.title).strong());
                                    ui.label(RichText::new(&game.source).small().color(MUTED))
                                        .on_hover_text(&game.reason);
                                },
                            );
                            ui.label(
                                RichText::new(game.outcome.label())
                                    .color(outcome_color(game.outcome)),
                            )
                            .on_hover_text(&game.reason);
                            if game.has_key
                                && ui
                                    .small_button("Show key")
                                    .on_hover_text("Reveal this stored key locally for 20 seconds")
                                    .clicked()
                            {
                                self.dispatch(
                                    Command::RevealKey {
                                        game_id: game.id.clone(),
                                    },
                                    "Unlocking key locally…",
                                );
                            }
                        });
                        ui.separator();
                    });
                }
            });
    }
    fn login_panel(&mut self, ui: &mut egui::Ui) {
        let mut cancel = false;
        let mut retry = false;
        if let Some(login) = &self.login {
            ui.horizontal(|ui| {
                ui.strong(format!("Sign in to {}", login.provider.name()));
                if ui.button("Close sign-in").clicked() {
                    cancel = true;
                }
                if ui.button("Verify sign-in again").clicked() {
                    retry = true;
                }
            });
            ui.label(RichText::new(login.provider.origin()).color(ACCENT));
            ui.label(&login.message);
            ui.label("After verification this view closes. Approved work continues without a browser window.");
            self.login_rect = ui.available_rect_before_wrap();
        }
        if retry && let Some(login) = &mut self.login {
            login.retry();
        }
        if cancel {
            if let Some(login) = &self.login {
                self.dismissed_logins.insert(login.provider);
            }
            self.login = None;
            self.login_requested = None;
        }
    }
    fn choices(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(
                "Claiming a game on Humble and activating its key on Steam are separate steps.",
            )
            .color(MUTED),
        );
        let pending = self.snapshot.as_ref().map(|s| {
            s.games
                .iter()
                .filter(|g| g.is_claimed_choice() && g.outcome.eligible())
                .count()
        });
        card(ui, |ui| {
            ui.strong("Already claimed on Humble");
            if let Some(pending) = pending {
                ui.label(format!("{pending} eligible claimed Choice keys still need review or Steam activation. Unrevealed keys require reveal permission in the batch review."));
            } else {
                ui.label("Your library is still loading. Respond to any Keychain prompt before reviewing saved Choice keys.");
            }
            if ui
                .add_enabled(
                    pending.is_some(),
                    egui::Button::new("View claimed Choice keys awaiting Steam"),
                )
                .clicked()
            {
                self.show_claimed_choice_keys();
            }
        });
        ui.add_space(14.);
        ui.heading("Unclaimed Choice selections");
        ui.label(RichText::new("Select Steam games from existing paid entitlements. Monthly limits apply; purchases and promotional memberships are excluded.").color(MUTED));
        ui.add_space(14.);
        let choices = self
            .snapshot
            .as_ref()
            .map(|s| s.choices.clone())
            .unwrap_or_default();
        if choices.is_empty() {
            ui.label("No unclaimed eligible Choice items were found. Synchronize to check again.");
            return;
        }
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut self.allow_reveal,
                "Allow selected claims and key reveals",
            );
            if primary(
                ui,
                "Review Choice selections",
                !self.choice_selected.is_empty() && self.rx.is_none(),
            ) {
                self.plan();
            }
        });
        ui.add_space(12.);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for choice in choices {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut checked = self.choice_selected.contains(&choice.id);
                        if ui
                            .add_enabled(
                                choice.available,
                                egui::Checkbox::new(&mut checked, &choice.title),
                            )
                            .changed()
                        {
                            if checked {
                                self.choice_selected.insert(choice.id.clone());
                            } else {
                                self.choice_selected.remove(&choice.id);
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} selections left",
                                    choice.month, choice.remaining
                                ))
                                .small()
                                .color(MUTED),
                            );
                        });
                    });
                });
                ui.add_space(8.);
            }
        });
    }
    fn show_claimed_choice_keys(&mut self) {
        self.page = Page::Library;
        self.only_claimed_choice = true;
        self.only_eligible = true;
        self.search.clear();
        self.selected.clear();
        self.choice_selected.clear();
    }
    fn review(&mut self, ui: &mut egui::Ui) {
        let Some(batch) = self.review.clone() else {
            ui.label("Select games in Library or Humble Choice, then prepare a batch for review.");
            return;
        };
        ui.label(RichText::new("This approval covers only these selected items and the currently connected accounts.").color(MUTED));
        ui.add_space(16.);
        card(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} library items · {} Choice selections",
                    batch.game_ids.len(),
                    batch.choice_ids.len()
                ))
                .size(22.)
                .strong(),
            );
            ui.label(if batch.allow_reveal {
                "Key reveals and selected Choice claims are permitted for this batch."
            } else {
                "This batch uses revealed keys only."
            });
            ui.label(RichText::new("Revealing a key removes its gift-link option. Activations cannot be undone by this app.").color(Color32::from_rgb(242,193,114)));
        });
        ui.add_space(15.);
        let snapshot = self.snapshot.clone();
        egui::ScrollArea::vertical()
            .max_height(330.)
            .show(ui, |ui| {
                if let Some(snapshot) = &snapshot {
                    for id in &batch.game_ids {
                        if let Some(game) = snapshot.games.iter().find(|g| &g.id == id) {
                            ui.label(format!("• {}", game.title));
                        }
                    }
                    for id in &batch.choice_ids {
                        if let Some(choice) = snapshot.choices.iter().find(|c| &c.id == id) {
                            ui.label(format!("• {} — {}", choice.title, choice.month));
                        }
                    }
                }
            });
        ui.add_space(20.);
        ui.monospace(format!("Batch {}", batch.id));
        ui.label(RichText::new(format!("State: {}", batch.state)).color(MUTED));
        ui.add_space(12.);
        let can_start = batch.state == "review"
            || (batch.approved && ["approved", "paused"].contains(&batch.state.as_str()));
        let action = if batch.approved {
            "Start approved batch"
        } else {
            "Approve this batch and start"
        };
        if can_start && primary(ui, action, self.rx.is_none()) {
            self.approve_start(batch);
            self.page = Page::Queue;
        }
    }
    fn queue(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Approved work continues when this window closes. Use Pause or Quit and pause to stop it safely.").color(MUTED));
        ui.add_space(16.);
        let batches = self
            .snapshot
            .as_ref()
            .map(|s| s.batches.clone())
            .unwrap_or_default();
        if batches.is_empty() {
            ui.label("No batches yet. Select titles in your library and review a batch.");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for batch in batches.iter().rev() {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Batch {}", &batch.id[..8.min(batch.id.len())]))
                                .size(18.)
                                .strong(),
                        );
                        ui.label(RichText::new(&batch.state).color(ACCENT));
                    });
                    ui.add(
                        egui::ProgressBar::new(if batch.game_ids.is_empty() {
                            0.
                        } else {
                            batch.cursor as f32 / batch.game_ids.len() as f32
                        })
                        .text(format!(
                            "{} / {} keys processed · {} / {} Choice claims",
                            batch.cursor,
                            batch.game_ids.len(),
                            batch.choice_cursor,
                            batch.choice_ids.len()
                        )),
                    );
                    if let Some(deadline) = batch.next_retry_at {
                        ui.label(format!(
                            "Cooldown remaining: {} minutes",
                            ((deadline - now()).max(0) + 59) / 60
                        ));
                    }
                    ui.label(RichText::new(&batch.message).small().color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Review items").clicked() {
                            self.review = Some(batch.clone());
                            self.page = Page::Review;
                        }
                        if ["running", "cooldown"].contains(&batch.state.as_str())
                            && ui.button("Pause").clicked()
                        {
                            self.dispatch(
                                Command::Pause {
                                    batch_id: batch.id.clone(),
                                },
                                "Pausing…",
                            );
                        }
                        if batch.state == "paused" && ui.button("Resume approved batch").clicked() {
                            self.dispatch(
                                Command::Resume {
                                    batch_id: batch.id.clone(),
                                },
                                "Resuming…",
                            );
                        }
                        if !["complete", "cancelled"].contains(&batch.state.as_str())
                            && ui.button("Cancel remaining items").clicked()
                        {
                            self.dispatch(
                                Command::Cancel {
                                    batch_id: batch.id.clone(),
                                },
                                "Cancelling…",
                            );
                        }
                    });
                });
                ui.add_space(12.);
            }
        });
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("PRIVATE STORAGE").small().color(MUTED));
        ui.add_space(10.);
        card(ui, |ui| {
            let persistent = self.snapshot.as_ref().is_some_and(|s| s.persistent_vault);
            ui.label(RichText::new(if self.demo{"Demo storage is separate from your real accounts."}else if persistent{"System keyring available. Sessions and keys are encrypted at rest."}else{"Session-only vault. Unlock your system keyring before reconnecting for persistent storage."}).strong());
            ui.label(RichText::new("No telemetry. Account passwords stay on official websites. Product keys and cookies never appear in assistant tool responses.").color(MUTED));
        });
        ui.add_space(20.);
        ui.label(RichText::new("BROWSER SIGN-IN").small().color(MUTED));
        egui::ComboBox::from_label("Browser")
            .selected_text(self.browser.map(|b| b.name()).unwrap_or("Automatic"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.browser, None, "Automatic");
                ui.selectable_value(&mut self.browser, Some(BrowserKind::Firefox), "Firefox");
                ui.selectable_value(&mut self.browser, Some(BrowserKind::Chrome), "Chrome");
            });
        ui.horizontal(|ui| {
            for browser in [BrowserKind::Firefox, BrowserKind::Chrome] {
                if ui
                    .button(format!("Provision {} driver", browser.name()))
                    .clicked()
                {
                    self.dispatch(
                        Command::InstallDriver { browser },
                        "Downloading and verifying the official driver…",
                    );
                }
            }
        });
        ui.add_space(20.);
        ui.label(
            RichText::new("LOCAL HISTORY AND REPORTS")
                .small()
                .color(MUTED),
        );
        if ui.button("Import legacy CSV history…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV history", &["csv"])
                .pick_file()
        {
            self.dispatch(
                Command::Import {
                    path: path.to_string_lossy().into(),
                },
                "Importing history locally…",
            );
        }
        ui.label(RichText::new("Original CSV files are preserved. Ambiguous old results retain their original uncertainty.").small().color(MUTED));
        ui.checkbox(
            &mut self.export_keys,
            "Include plaintext product keys in the next local export",
        );
        if self.export_keys {
            ui.colored_label(
                Color32::from_rgb(242, 193, 114),
                "This file will contain secrets. Keep it private and out of GitHub.",
            );
        }
        if ui.button("Save CSV report…").clicked() {
            self.export_dialog(self.export_keys);
            self.export_keys = false;
        }
        ui.add_space(20.);
        ui.label(RichText::new("ASSISTANT CONNECTION").small().color(MUTED));
        ui.label("The bundled hrctl mcp command exposes local, structured controls over stdio. Approve one concrete batch at a time.");
        ui.horizontal(|ui| {
            for provider in [Provider::Humble, Provider::Steam] {
                if ui
                    .button(format!("Forget {} session", provider.name()))
                    .clicked()
                {
                    self.dispatch(
                        Command::Disconnect { provider },
                        "Forgetting local session…",
                    );
                }
            }
        });
        ui.add_space(20.);
        if ui.button("Quit worker and pause approved work").clicked() {
            self.quit_after_response = true;
            self.dispatch(Command::Shutdown { pause: true }, "Stopping worker…");
        }
    }
    fn export_dialog(&mut self, include_keys: bool) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(if include_keys {
                "private-key-export.csv"
            } else {
                "redemption-report.csv"
            })
            .add_filter("CSV report", &["csv"])
            .save_file()
        {
            self.dispatch(
                Command::Export {
                    path: path.to_string_lossy().into(),
                    include_keys,
                },
                "Saving local report…",
            );
        }
    }
}
impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Dispose the child view while its native parent still exists.
        self.login = None;
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        self.receive();
        if self.quit_after_response && self.rx.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.rx.is_none() && self.last_poll.elapsed() > Duration::from_secs(3) {
            self.refresh();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.page = Page::Library;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("library-search")));
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::R)) {
            self.dispatch(Command::Sync, "Synchronizing…");
        }
        if self.login.is_none()
            && let Some(provider) = self.login_requested.take()
        {
            match login::Login::new(frame, provider, self.demo, self.login_fixture) {
                Ok(login) => self.login = Some(login),
                Err(error) => self.error = safe_text(&error.to_string()),
            }
        }
        if let Some(snapshot) = &self.snapshot {
            if self.login.is_none() && !self.demo {
                for (provider, connection) in [
                    (Provider::Humble, &snapshot.humble),
                    (Provider::Steam, &snapshot.steam),
                ] {
                    if connection.native_login && !self.dismissed_logins.contains(&provider) {
                        self.login_requested = Some(provider);
                        break;
                    }
                }
            }
            if self.login.as_ref().is_some_and(|login| {
                !login.fixture_mode()
                    && if login.provider == Provider::Humble {
                        snapshot.humble.connected
                    } else {
                        snapshot.steam.connected
                    }
            }) {
                self.login = None;
            }
        }
        if let Some(login) = &mut self.login
            && let Some(command) = login.poll(self.rx.is_none())
        {
            self.dispatch(command, "Verifying background session…");
        }
        self.render(ctx);
        if let Some(login) = &self.login {
            login.layout(self.login_rect, !self.login_rect.is_negative());
        }
        ctx.request_repaint_after(Duration::from_millis(150));
        self.frames += 1;
        if self.smoke && self.frames > 25 {
            let worker_ready = self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.demo && s.humble.connected);
            if worker_ready
                && (!self.login_fixture
                    || self.login.as_ref().is_some_and(|login| login.fixture_ready))
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.smoke_started.elapsed() > Duration::from_secs(30) {
                eprintln!("Synthetic native sign-in view did not become ready");
                if let Ok(root) = humble_redeemer_core::store::data_root(true) {
                    let diagnostics = serde_json::json!({
                        "error":safe_text(&self.error),
                        "login_message":self.login.as_ref().map(|login|safe_text(&login.message)),
                        "has_view":self.login.is_some(),
                        "worker_pending":self.rx.is_some(),
                        "snapshot_received":self.snapshot.is_some(),
                        "fixture_probe":self.login.as_ref().and_then(|login|login.fixture_diagnostics.as_ref()),
                    });
                    let _ = std::fs::write(
                        root.join("smoke-diagnostics.json"),
                        diagnostics.to_string(),
                    );
                }
                std::process::exit(2);
            }
        }
    }
}
fn card<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(Color32::from_rgb(25, 32, 43))
        .corner_radius(10.)
        .inner_margin(16.)
        .show(ui, body)
        .inner
}
fn primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(
            RichText::new(text)
                .strong()
                .color(Color32::from_rgb(8, 35, 29)),
        )
        .fill(ACCENT)
        .min_size(Vec2::new(140., 32.)),
    )
    .clicked()
}
fn outcome_color(outcome: Outcome) -> Color32 {
    match outcome {
        Outcome::Redeemed | Outcome::Ready => ACCENT,
        Outcome::Expired | Outcome::UsedElsewhere | Outcome::PermanentFailure => {
            Color32::from_rgb(246, 143, 153)
        }
        Outcome::UnknownOutcome | Outcome::Exhausted | Outcome::RateLimited => {
            Color32::from_rgb(242, 193, 114)
        }
        _ => MUTED,
    }
}
#[cfg(target_os = "macos")]
fn install_edit_menu() -> muda::Menu {
    use muda::{Menu, PredefinedMenuItem, Submenu};
    let menu = Menu::new();
    if let Ok(application) =
        Submenu::with_items("Humble Redeemer", true, &[&PredefinedMenuItem::quit(None)])
    {
        let _ = menu.append(&application);
    }
    if let Ok(edit) = Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    ) {
        let _ = menu.append(&edit);
    }
    menu.init_for_nsapp();
    menu
}
fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let smoke = args.iter().any(|a| a == "--smoke-test");
    let login_fixture = args.iter().any(|a| a == "--login-demo");
    let demo = smoke || login_fixture || args.iter().any(|a| a == "--demo");
    #[cfg(target_os = "linux")]
    gtk::gdk::set_allowed_backends("x11");
    #[cfg(target_os = "linux")]
    gtk::init().map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    #[cfg(target_os = "linux")]
    winit::platform::x11::register_xlib_error_hook(Box::new(|_, error| {
        // Wry's official winit integration handles this queued GLXBadWindow
        // error from WebKit's shared X11 connection the same way:
        // https://github.com/tauri-apps/wry/blob/dev/examples/winit.rs
        // SAFETY: winit supplies a valid XErrorEvent for this callback.
        unsafe { (*(error.cast::<x11_dl::xlib::XErrorEvent>())).error_code == 170 }
    }));
    eframe::run_native(
        if login_fixture {
            "Humble Redeemer — Demo sign-in"
        } else {
            "Humble Redeemer"
        },
        eframe::NativeOptions {
            // Direct3D also supports the software adapter on remote desktops
            // whose system OpenGL implementation is limited to version 1.1.
            #[cfg(windows)]
            renderer: eframe::Renderer::Wgpu,
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200., 820.])
                .with_min_inner_size([900., 650.]),
            #[cfg(target_os = "linux")]
            event_loop_builder: Some(Box::new(|builder| {
                use winit::platform::x11::EventLoopBuilderExtX11;
                builder.with_x11();
            })),
            ..Default::default()
        },
        Box::new(move |cc| {
            let mut style = (*cc.egui_ctx.style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.selection.bg_fill = Color32::from_rgb(36, 102, 90);
            style.spacing.item_spacing = Vec2::new(10., 8.);
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.));
            cc.egui_ctx.set_style(style);
            let mut app = App::new(demo, smoke);
            #[cfg(target_os = "macos")]
            {
                app.native_menu = Some(install_edit_menu());
            }
            if login_fixture {
                app.login_fixture = true;
                app.login_requested = Some(Provider::Humble);
            }
            Ok(Box::new(app))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claimed_choice_navigation_does_not_select_or_approve_any_games() {
        let mut app = App::new(true, false);
        app.search = "previous search".into();
        app.selected.insert("previous-library-selection".into());
        app.choice_selected
            .insert("previous-choice-selection".into());
        app.show_claimed_choice_keys();
        assert!(matches!(app.page, Page::Library));
        assert!(app.only_claimed_choice && app.only_eligible);
        assert!(app.search.is_empty() && app.selected.is_empty() && app.choice_selected.is_empty());
        assert!(app.rx.is_none() && app.review.is_none());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn glib_string_iterator_backport() {
        use gtk::glib::variant::ToVariant;
        let value = vec!["first", "middle", "last"].to_variant();
        let mut iter = value.array_iter_str().unwrap();
        assert_eq!(iter.next(), Some("first"));
        assert_eq!(iter.next_back(), Some("last"));
        assert_eq!(iter.last(), Some("middle"));
    }
    #[test]
    fn every_screen_renders_without_network_or_credentials() {
        let ctx = egui::Context::default();
        let mut app = App::new(true, false);
        let directory = tempfile::tempdir().unwrap();
        let engine = humble_redeemer_core::engine::Engine::new(
            humble_redeemer_core::store::Store::open(directory.path().into(), true).unwrap(),
            true,
        )
        .unwrap();
        app.snapshot = Some(engine.snapshot().unwrap());
        for page in [
            Page::Overview,
            Page::Library,
            Page::Choice,
            Page::Review,
            Page::Queue,
            Page::Results,
            Page::Settings,
        ] {
            app.page = page;
            let output = ctx.run(egui::RawInput::default(), |ctx| app.render(ctx));
            assert!(!output.shapes.is_empty());
        }
    }
}
