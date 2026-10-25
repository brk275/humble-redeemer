//! Explicit opt-in public-source probe. No account, cookies, claims or live vault.
use humble_redeemer_core::{
    psn::{Command, State},
    store::Store,
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let region = std::env::args().nth(1).unwrap_or_else(|| "en-us".into());
    let directory = tempfile::tempdir()?;
    let mut store = Store::open(directory.path().into(), true)?;
    let mut state = State::load(&store, false)?;
    state
        .handle(
            &mut store,
            Command::Configure {
                region,
                notifications: false,
            },
            false,
        )
        .await?;
    state.refresh(&mut store).await?;
    println!(
        "{}",
        serde_json::json!({"offers":state.offers.len(),"sources":state.sources,"categories":state.offers.iter().map(|o|o.category).collect::<Vec<_>>()})
    );
    Ok(())
}
