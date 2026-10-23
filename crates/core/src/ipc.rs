use crate::{
    engine::Engine,
    model::Command,
    privacy::safe_text,
    store::{Store, data_root, private_dir},
};
use anyhow::{Result, anyhow, bail};
use fs2::FileExt;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    ListenerOptions, Name,
    tokio::{Stream, prelude::*},
};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};

#[derive(Serialize, Deserialize)]
struct Envelope {
    token: String,
    command: Command,
}
fn trace(stage: &str) {
    if std::env::var("HUMBLE_REDEEMER_TRACE").as_deref() == Ok("1") {
        eprintln!("Local IPC: {stage}");
        // Opt-in synthetic diagnostics contain fixed stage labels only.
        if let Some(root) = std::env::var_os("HUMBLE_REDEEMER_DATA_DIR") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(Path::new(&root).join("worker-trace.log"))
            {
                let _ = writeln!(file, "{stage}");
            }
        }
    }
}
fn socket_name(root: &Path) -> Result<Name<'static>> {
    #[cfg(unix)]
    {
        Ok(root
            .join("worker.sock")
            .to_fs_name::<GenericFilePath>()?
            .into_owned())
    }
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        let identifier = format!(
            "humble-redeemer-{}",
            &hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()))[..24]
        );
        Ok(identifier.to_ns_name::<GenericNamespaced>()?.into_owned())
    }
}
fn token(root: &Path) -> Result<String> {
    std::fs::read_to_string(root.join("control.token")).map_err(|_| anyhow!("Worker is not ready"))
}
fn write_token(root: &Path) -> Result<()> {
    let path = root.join("control.token");
    if path.exists() {
        return Ok(());
    }
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)?
        .write_all(hex::encode(bytes).as_bytes())?;
    Ok(())
}
pub async fn request(demo: bool, command: Command) -> Result<Value> {
    request_at(&data_root(demo)?, command).await
}
pub async fn request_at(root: &Path, command: Command) -> Result<Value> {
    trace("connecting");
    let conn = Stream::connect(socket_name(root)?)
        .await
        .map_err(|_| anyhow!("Worker unavailable. Start Humble Redeemer or run hrctl worker."))?;
    trace("connected");
    let mut sender = &conn;
    let mut bytes = serde_json::to_vec(&Envelope {
        token: token(root)?,
        command,
    })?;
    bytes.push(b'\n');
    sender.write_all(&bytes).await?;
    sender.flush().await?;
    trace("request written");
    let mut reader = BufReader::new(&conn);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(300), reader.read_line(&mut line))
        .await
        .map_err(|_| {
            anyhow!("Operation is still running; check status before trying it again")
        })??;
    trace("response read");
    let value: Value =
        serde_json::from_str(&line).map_err(|_| anyhow!("Worker returned an invalid response"))?;
    if value["ok"] != true {
        bail!("{}", value["error"].as_str().unwrap_or("Operation failed"));
    }
    Ok(value["result"].clone())
}
pub async fn ensure_worker(demo: bool) -> Result<()> {
    trace("checking worker");
    if request(demo, Command::Snapshot).await.is_ok() {
        return Ok(());
    }
    let current = std::env::current_exe()?;
    let binary = if current.file_stem().is_some_and(|s| s == "hrctl") {
        current
    } else {
        current
            .parent()
            .ok_or_else(|| anyhow!("Application location unavailable"))?
            .join(format!("hrctl{}", std::env::consts::EXE_SUFFIX))
    };
    let mut command = std::process::Command::new(binary);
    if demo {
        command.arg("--demo");
    }
    command
        .arg("worker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.spawn().map_err(|_| {
        anyhow!("Could not launch the bundled worker. Ensure hrctl is installed beside the GUI.")
    })?;
    trace("worker spawned");
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if request(demo, Command::Snapshot).await.is_ok() {
            return Ok(());
        }
    }
    bail!("Worker did not start. Unlock your system keyring and try again.")
}
pub async fn serve(demo: bool) -> Result<()> {
    trace("worker initializing");
    let root = data_root(demo)?;
    private_dir(&root)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("worker.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow!("A worker is already running for this application profile"))?;
    write_token(&root)?;
    #[cfg(unix)]
    {
        let path = root.join("worker.sock");
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    let listener = ListenerOptions::new()
        .name(socket_name(&root)?)
        .create_tokio()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("worker.sock"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    let expected = token(&root)?;
    let engine = Arc::new(Mutex::new(Engine::new(Store::open(root, demo)?, demo)?));
    trace("worker ready");
    let tick_engine = engine.clone();
    let ticking = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut engine = tick_engine.lock().await;
            if engine.shutdown {
                break;
            }
            if let Err(error) = engine.tick().await {
                engine.message = safe_text(&error.to_string());
                let _ = engine.store.recover();
            }
        }
    });
    loop {
        tokio::select! {
            result=listener.accept()=>{
                let conn=result?;let engine=engine.clone();let expected=expected.clone();
                tokio::spawn(async move{let _=handle_connection(conn,engine,expected).await;});
            }
            _=tokio::time::sleep(Duration::from_millis(200))=>{if engine.lock().await.shutdown{break}}
        }
    }
    ticking.abort();
    drop(listener);
    drop(lock);
    Ok(())
}
async fn handle_connection(
    conn: Stream,
    engine: Arc<Mutex<Engine>>,
    expected: String,
) -> Result<()> {
    let mut receiver = BufReader::new(&conn).take(65536);
    trace("worker accepted connection");
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(15),
        receiver.read_until(b'\n', &mut bytes),
    )
    .await??;
    trace("worker read request");
    if bytes.len() >= 65536 {
        return Ok(());
    }
    let result = match serde_json::from_slice::<Envelope>(&bytes) {
        Ok(envelope) if constant_time_equal(envelope.token.as_bytes(), expected.as_bytes()) => {
            let mut engine = engine.lock().await;
            match engine.handle(envelope.command).await {
                Ok(result) => json!({"ok":true,"result":result}),
                Err(error) => {
                    let message = safe_text(&error.to_string());
                    engine.message = message.clone();
                    json!({"ok":false,"error":message})
                }
            }
        }
        _ => json!({"ok":false,"error":"Local control authentication failed"}),
    };
    let mut bytes = serde_json::to_vec(&result)?;
    bytes.push(b'\n');
    let mut sender = &conn;
    sender.write_all(&bytes).await?;
    sender.flush().await?;
    Ok(())
}
fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (left, right) in a.iter().zip(b) {
        diff |= (left ^ right) as usize;
    }
    diff == 0
}
pub fn blocking_request(demo: bool, command: Command) -> Result<Value> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        if !matches!(command, Command::Shutdown { .. }) {
            ensure_worker(demo).await?;
        }
        request(demo, command).await
    })
}
pub fn executable_directory() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow!("Application path unavailable"))?
        .to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_comparison() {
        assert!(constant_time_equal(b"fixture", b"fixture"));
        assert!(!constant_time_equal(b"fixture", b"fixtur"));
        assert!(!constant_time_equal(b"fixture", b"fixturex"));
    }
}
