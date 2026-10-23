mod mcp;
use clap::{Parser, Subcommand};
use humble_redeemer_core::{ipc, model::*, privacy::safe_text};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(version, about = "Private local controls for Humble Redeemer")]
struct Args {
    #[arg(long, global = true)]
    demo: bool,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Status,
    Worker,
    Mcp,
    Sync,
    Connect {
        #[arg(value_parser=["humble","steam"])]
        provider: String,
        #[arg(long,value_parser=["firefox","chrome"])]
        browser: Option<String>,
    },
    Disconnect {
        #[arg(value_parser=["humble","steam"])]
        provider: String,
    },
    Library {
        #[arg(long, default_value = "")]
        search: String,
    },
    Choices,
    Plan {
        #[arg(long, value_delimiter = ',')]
        games: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        choices: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        allow_reveal: bool,
    },
    Batch {
        #[command(subcommand)]
        command: BatchAction,
    },
    Import {
        path: String,
    },
    Export {
        path: String,
        #[arg(long)]
        include_keys: bool,
    },
    Driver {
        #[arg(value_parser=["firefox","chrome"])]
        browser: String,
    },
    Retry {
        game_id: String,
    },
    Quit {
        #[arg(long)]
        pause: bool,
    },
}
#[derive(Subcommand)]
enum BatchAction {
    Approve {
        id: String,
        #[arg(long)]
        digest: String,
    },
    Start {
        id: String,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Cancel {
        id: String,
    },
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(Args::parse()).await {
        eprintln!("{}", json!({"error":safe_text(&error.to_string())}));
        std::process::exit(1);
    }
}
async fn run(args: Args) -> anyhow::Result<()> {
    if matches!(args.command, Action::Worker) {
        return ipc::serve(args.demo).await;
    }
    if matches!(args.command, Action::Mcp) {
        return mcp::serve(args.demo).await;
    }
    if !matches!(args.command, Action::Quit { .. }) {
        ipc::ensure_worker(args.demo).await?;
    }
    let command = match args.command {
        Action::Status | Action::Library { .. } | Action::Choices => Command::Snapshot,
        Action::Connect {
            ref provider,
            ref browser,
        } => Command::Connect {
            provider: parse_provider(provider),
            browser: browser.as_deref().map(parse_browser),
        },
        Action::Disconnect { ref provider } => Command::Disconnect {
            provider: parse_provider(provider),
        },
        Action::Sync => Command::Sync,
        Action::Plan {
            ref games,
            ref choices,
            all,
            allow_reveal,
        } => {
            let ids = if all {
                let snapshot: Snapshot =
                    serde_json::from_value(ipc::request(args.demo, Command::Snapshot).await?)?;
                snapshot
                    .games
                    .into_iter()
                    .filter(|g| g.outcome.eligible())
                    .map(|g| g.id)
                    .collect()
            } else {
                games.clone()
            };
            Command::Plan {
                game_ids: ids,
                choice_ids: choices.clone(),
                allow_reveal,
            }
        }
        Action::Batch { ref command } => match command {
            BatchAction::Approve { id, digest } => Command::Approve {
                batch_id: id.clone(),
                digest: digest.clone(),
            },
            BatchAction::Start { id } => Command::Start {
                batch_id: id.clone(),
            },
            BatchAction::Pause { id } => Command::Pause {
                batch_id: id.clone(),
            },
            BatchAction::Resume { id } => Command::Resume {
                batch_id: id.clone(),
            },
            BatchAction::Cancel { id } => Command::Cancel {
                batch_id: id.clone(),
            },
        },
        Action::Import { ref path } => Command::Import { path: path.clone() },
        Action::Export {
            ref path,
            include_keys,
        } => Command::Export {
            path: path.clone(),
            include_keys,
        },
        Action::Driver { ref browser } => Command::InstallDriver {
            browser: parse_browser(browser),
        },
        Action::Retry { ref game_id } => Command::Retry {
            game_id: game_id.clone(),
        },
        Action::Quit { pause } => Command::Shutdown { pause },
        Action::Worker | Action::Mcp => unreachable!(),
    };
    let mut value = ipc::request(args.demo, command).await?;
    match args.command {
        Action::Library { search } => {
            let query = search.to_lowercase();
            let games = value["games"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|g| {
                    g["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                })
                .collect::<Vec<Value>>();
            value = json!({"games":games});
        }
        Action::Choices => value = json!({"choices":value["choices"]}),
        _ => {}
    }
    println!(
        "{}",
        if args.json {
            serde_json::to_string(&value)?
        } else {
            serde_json::to_string_pretty(&value)?
        }
    );
    Ok(())
}
fn parse_provider(value: &str) -> Provider {
    if value == "steam" {
        Provider::Steam
    } else {
        Provider::Humble
    }
}
fn parse_browser(value: &str) -> BrowserKind {
    if value == "chrome" {
        BrowserKind::Chrome
    } else {
        BrowserKind::Firefox
    }
}
