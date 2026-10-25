use super::*;
use humble_redeemer_core::psn::{self, Category, Confirmation, OpenRequest};
#[derive(Default)]
pub struct UiState {
    pub region: String,
    pub notifications: bool,
    pub settings_loaded: bool,
    pub selected: HashSet<String>,
    pub search: String,
    pub category: Option<Category>,
    pub platform: String,
    pub unclaimed: bool,
    pub expiring: bool,
    pub view: Option<psn_view::ManualView>,
    pub request: Option<OpenRequest>,
    pub pending: Option<(OpenRequest, Vec<Value>)>,
    pub last_request: Option<String>,
    pub rect: Option<egui::Rect>,
    pub browser_error: String,
    pub fixture: bool,
    pub track_title: String,
    pub track_url: String,
    pub track_category: Option<Category>,
}
impl App {
    pub fn psn_page(&mut self, ui: &mut egui::Ui) {
        let Some(state) = self.snapshot.as_ref().map(|s| s.psn.clone()) else {
            ui.spinner();
            return;
        };
        ui.label(RichText::new("Discover public offers. Complete claims on Sony’s official page, then confirm here.").color(MUTED));
        if !self.psn.settings_loaded {
            self.psn.region = state.config.region.clone();
            self.psn.notifications = state.config.notifications;
            self.psn.settings_loaded = true;
        }
        if let Some(request) = self.psn.request.clone() {
            let title = state
                .offers
                .iter()
                .find(|o| o.id == request.offer_id)
                .map(|o| o.title.as_str())
                .unwrap_or("PlayStation offer");
            ui.add_space(8.);
            ui.label(RichText::new(title).strong());
            ui.label("You control this page. Check the exact edition, requirements and a zero-cost checkout. The app never clicks Add to Library or subscribes for you.");
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!self.demo, egui::Button::new("Open in normal browser"))
                    .clicked()
                {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(request.url.clone()));
                    self.psn.view = None;
                }
                if ui.button("Save session and close").clicked() {
                    self.finish_psn(None);
                }
            });
            if !self.psn.browser_error.is_empty() {
                ui.colored_label(Color32::from_rgb(242, 193, 114), &self.psn.browser_error);
            }
            ui.label("After checking PlayStation’s result, record a user-confirmed outcome:");
            ui.horizontal_wrapped(|ui| {
                for outcome in [
                    Confirmation::Claimed,
                    Confirmation::AlreadyOwned,
                    Confirmation::Skipped,
                    Confirmation::CouldNotClaim,
                ] {
                    if ui
                        .add_enabled(self.rx.is_none(), egui::Button::new(outcome.label()))
                        .clicked()
                    {
                        self.finish_psn(Some(outcome));
                    }
                }
            });
            if self.psn.view.is_some() {
                let rect = ui.available_rect_before_wrap();
                self.psn.rect = Some(rect.shrink(4.));
                ui.allocate_rect(rect, egui::Sense::hover());
            }
            return;
        }
        egui::CollapsingHeader::new("Store region and reminders").default_open(state.config.region.is_empty()).show(ui,|ui|{
            ui.label("Choose the locale matching your Sony account. This does not change the account’s region. Local history labels who observed each result.");
            ui.horizontal(|ui|{
                ui.add(egui::TextEdit::singleline(&mut self.psn.region).hint_text("Store locale, e.g. en-us").desired_width(180.));
                egui::ComboBox::from_id_salt("psn-region").selected_text("Examples").show_ui(ui,|ui|for locale in ["en-us","en-ca","en-gb","de-de","fr-fr","ja-jp","en-au"]{if ui.selectable_label(false,locale).clicked(){self.psn.region=locale.into();}});
            });
            ui.checkbox(&mut self.psn.notifications,"Notify me about known claim deadlines on this computer");
            if primary(ui,"Save PSN settings",self.rx.is_none()){self.dispatch(Command::Psn{command:psn::Command::Configure{region:self.psn.region.trim().to_lowercase(),notifications:self.psn.notifications}},"Saving PlayStation settings…");}
            if ui.button("Forget this region’s saved PSN browser session").clicked(){self.dispatch(Command::Psn{command:psn::Command::ForgetSession},"Forgetting only the PSN browser session…");}
        });
        if state.config.region.is_empty() {
            ui.label("Set your store locale to begin. No PSN sign-in is required to browse public offers.");
            return;
        }
        let gaps = state.sources.iter().filter(|s| s.issue.is_some()).count();
        if gaps > 0 {
            ui.colored_label(Color32::from_rgb(242,193,114),format!("Coverage incomplete: {gaps} public sources could not expose offer data. Cached results and manual official links remain available."));
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Store: {}", state.config.region));
            if ui
                .add_enabled(
                    self.rx.is_none(),
                    egui::Button::new("Refresh public offers"),
                )
                .clicked()
            {
                self.dispatch(
                    Command::Psn {
                        command: psn::Command::Refresh,
                    },
                    "Reading official public PlayStation pages…",
                );
            }
            if primary(
                ui,
                "Prepare selected checklist",
                self.rx.is_none() && !self.psn.selected.is_empty(),
            ) {
                let ids = self.psn.selected.iter().cloned().collect();
                self.dispatch(
                    Command::Psn {
                        command: psn::Command::Plan { offer_ids: ids },
                    },
                    "Preparing a user-operated checklist…",
                );
                self.psn.selected.clear();
            }
        });
        egui::CollapsingHeader::new("Track an official offer link").show(ui,|ui|{
            ui.label("Use this when public discovery misses an offer. Price and eligibility remain Unknown until you check Sony’s page.");
            ui.add(egui::TextEdit::singleline(&mut self.psn.track_title).hint_text("Title"));
            ui.add(egui::TextEdit::singleline(&mut self.psn.track_url).hint_text("Official product or game URL for your store region"));
            egui::ComboBox::from_id_salt("psn-track-kind").selected_text(self.psn.track_category.unwrap_or(Category::Unknown).label()).show_ui(ui,|ui|for category in Category::ALL{ui.selectable_value(&mut self.psn.track_category,Some(category),category.label());});
            if ui.add_enabled(self.rx.is_none(),egui::Button::new("Track this link")).clicked(){self.dispatch(Command::Psn{command:psn::Command::Track{url:self.psn.track_url.clone(),title:self.psn.track_title.clone(),category:self.psn.track_category.unwrap_or(Category::Unknown)}},"Tracking a public PlayStation link…");}
        });
        if let Some(issue) = &state.notification_issue {
            ui.colored_label(Color32::from_rgb(242, 193, 114), issue);
        }
        egui::CollapsingHeader::new("Claim checklists")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("psn-lists")
                    .max_height(180.)
                    .show(ui, |ui| {
                        for list in state
                            .checklists
                            .iter()
                            .rev()
                            .filter(|l| l.region == state.config.region)
                        {
                            ui.label(format!(
                                "Checklist {} · {} offers",
                                &list.id[..8.min(list.id.len())],
                                list.offer_ids.len()
                            ));
                            for id in &list.offer_ids {
                                let Some(offer) = state.offers.iter().find(|o| &o.id == id) else {
                                    continue;
                                };
                                let record = state
                                    .history
                                    .iter()
                                    .rev()
                                    .find(|r| r.checklist_id == list.id && r.offer_id == *id);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(&offer.title);
                                    if let Some(r) = record {
                                        ui.label(format!(
                                            "{} · {}",
                                            r.outcome.label(),
                                            r.recorded_by.label()
                                        ));
                                    }
                                    if ui
                                        .add_enabled(
                                            self.rx.is_none(),
                                            egui::Button::new(if record.is_some() {
                                                "Open again"
                                            } else {
                                                "Open official claim page"
                                            }),
                                        )
                                        .clicked()
                                    {
                                        self.dispatch(
                                            Command::Psn {
                                                command: psn::Command::Open {
                                                    checklist_id: list.id.clone(),
                                                    offer_id: id.clone(),
                                                },
                                            },
                                            "Opening an official page for you…",
                                        );
                                    }
                                });
                            }
                        }
                    });
            });
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.psn.search)
                    .id(egui::Id::new("psn-search"))
                    .hint_text("Search PSN titles or history…")
                    .desired_width(230.),
            );
            egui::ComboBox::from_id_salt("psn-category")
                .selected_text(
                    self.psn
                        .category
                        .map(Category::label)
                        .unwrap_or("All categories"),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.psn.category, None, "All categories");
                    for c in Category::ALL {
                        ui.selectable_value(&mut self.psn.category, Some(c), c.label());
                    }
                });
            egui::ComboBox::from_id_salt("psn-platform")
                .selected_text(if self.psn.platform.is_empty() {
                    "All platforms"
                } else {
                    &self.psn.platform
                })
                .show_ui(ui, |ui| {
                    for p in ["", "PS4", "PS5"] {
                        ui.selectable_value(
                            &mut self.psn.platform,
                            p.into(),
                            if p.is_empty() { "All platforms" } else { p },
                        );
                    }
                });
            ui.checkbox(&mut self.psn.unclaimed, "Hide recorded claims");
            ui.checkbox(&mut self.psn.expiring, "Ends within 72 hours");
        });
        egui::ScrollArea::vertical().show(ui,|ui|{
            let mut count=0;
            for o in state.offers.iter().filter(|o|o.region==state.config.region) {
                if !o.title.to_lowercase().contains(&self.psn.search.to_lowercase())||self.psn.category.is_some_and(|c|c!=o.category)||(!self.psn.platform.is_empty()&&!o.platforms.contains(&self.psn.platform))|| (self.psn.expiring&&!o.deadline.is_some_and(|t|t>now()&&t-now()<=72*3600))|| (self.psn.unclaimed&&state.history.iter().any(|r|r.offer_id==o.id&&matches!(r.outcome,Confirmation::Claimed|Confirmation::AlreadyOwned))){continue;}
                count+=1;card(ui,|ui|{
                    let mut selected=self.psn.selected.contains(&o.id);if ui.checkbox(&mut selected,&o.title).changed(){if selected{self.psn.selected.insert(o.id.clone());}else{self.psn.selected.remove(&o.id);}}
                    if let Some(record)=state.history.iter().rev().find(|r|r.offer_id==o.id){ui.label(format!("{} · {}",record.outcome.label(),record.recorded_by.label()));}
                    if ui.add_enabled(self.rx.is_none(),egui::Button::new("Open claim page")).clicked(){self.open_psn_offer(o.id.clone());}
                    ui.label(format!("{} · {} · Edition: {}",o.category.label(),o.price.as_deref().unwrap_or("Price unknown — verify on PlayStation"),o.edition.as_deref().unwrap_or("Unknown")));
                    ui.label(format!("Platforms: {} · Subscription: {} · Base game needed: {}",if o.platforms.is_empty(){"Unknown".into()}else{o.platforms.join(", ")},known(o.subscription),known(o.base_game)));
                    ui.label(&o.entitlement);
                    ui.label(format!("Claim deadline: {} · Last public check: {}",o.deadline.map(stamp).unwrap_or_else(||"Unknown".into()),stamp(o.checked_at)));
                    ui.hyperlink_to("Official PlayStation page",&o.url);
                });ui.add_space(6.);
            }
            if count==0{ui.label("No matching offers. Check source coverage below, adjust filters, or refresh.");}
            egui::CollapsingHeader::new("Public sources and coverage").default_open(true).show(ui,|ui|{
                ui.label("Discovery may be incomplete. Public prices do not prove account eligibility or ownership. Missing fields stay Unknown.");
                for source in &state.sources {ui.hyperlink_to(&source.name,&source.url);ui.label(format!("{} offers · Last successful read: {}",source.count,source.last_success.map(stamp).unwrap_or_else(||"Unknown".into())));if let Some(issue)=&source.issue{ui.colored_label(Color32::from_rgb(242,193,114),issue);}}
            });
            egui::CollapsingHeader::new("Claim history").show(ui,|ui|for record in state.history.iter().rev().filter(|r|r.region==state.config.region&&r.title.to_lowercase().contains(&self.psn.search.to_lowercase())){
                ui.label(format!("{} · {} · {} · {}",record.title,record.outcome.label(),record.recorded_by.label(),stamp(record.at)));ui.small(&record.evidence);
            });
        });
    }
    fn open_psn_offer(&mut self, offer_id: String) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let demo = self.demo;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Value> {
                let planned: Snapshot = serde_json::from_value(ipc::blocking_request(
                    demo,
                    Command::Psn {
                        command: psn::Command::Plan {
                            offer_ids: vec![offer_id.clone()],
                        },
                    },
                )?)?;
                let list = planned
                    .psn
                    .checklists
                    .last()
                    .ok_or_else(|| anyhow::anyhow!("Checklist not returned"))?;
                if list.offer_ids != vec![offer_id.clone()] {
                    anyhow::bail!("The checklist selection changed; open the offer again");
                }
                ipc::blocking_request(
                    demo,
                    Command::Psn {
                        command: psn::Command::Open {
                            checklist_id: list.id.clone(),
                            offer_id,
                        },
                    },
                )
            })()
            .map_err(|e| safe_text(&e.to_string()));
            let _ = tx.send(result);
        });
        self.rx = Some(rx);
        self.busy = "Opening the official claim page…".into();
    }
    fn finish_psn(&mut self, outcome: Option<Confirmation>) {
        if self.rx.is_some() {
            return;
        }
        let Some(request) = self.psn.request.clone() else {
            return;
        };
        let cookies = self.psn.view.as_ref().and_then(|v| v.cookies().ok());
        let (tx, rx) = mpsc::channel();
        let demo = self.demo;
        std::thread::spawn(move || {
            let result = (|| {
                if let Some(cookies) = cookies {
                    ipc::blocking_request(
                        demo,
                        Command::Psn {
                            command: psn::Command::SaveBrowserSession {
                                request_id: request.id.clone(),
                                cookies,
                            },
                        },
                    )?;
                }
                ipc::blocking_request(
                    demo,
                    Command::Psn {
                        command: if let Some(outcome) = outcome {
                            psn::Command::Confirm {
                                request_id: request.id,
                                outcome,
                            }
                        } else {
                            psn::Command::Close {
                                request_id: request.id,
                            }
                        },
                    },
                )
            })()
            .map_err(|e: anyhow::Error| safe_text(&e.to_string()));
            let _ = tx.send(result);
        });
        self.psn.view = None;
        self.psn.request = None;
        self.rx = Some(rx);
        self.busy = "Saving your local PlayStation record…".into();
    }
}
fn known(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "Unknown",
    }
}
fn stamp(value: i64) -> String {
    humble_redeemer_core::psn::format_time(value)
}
