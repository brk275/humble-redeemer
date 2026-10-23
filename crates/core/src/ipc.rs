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
    verify_peer(&conn)?;
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let mut command = std::process::Command::new(&binary);
        if demo {
            command.arg("--demo");
        }
        command
            .arg("worker")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.process_group(0);
        command.spawn().map_err(|_| {
            anyhow!(
                "Could not launch the bundled worker. Ensure hrctl is installed beside the GUI."
            )
        })?;
    }
    #[cfg(windows)]
    windows_peer::spawn_worker(&binary, demo)?;
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
    let options = ListenerOptions::new().name(socket_name(&root)?);
    #[cfg(windows)]
    let options = {
        use interprocess::os::windows::{
            local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
        };
        // Protected owner-only DACL; capability authentication remains required.
        let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;OW)")?;
        options.security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
    };
    let listener = options.create_tokio()?;
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
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            result=listener.accept()=>{
                let conn=result?;let engine=engine.clone();let expected=expected.clone();
                connections.spawn(async move{let _=handle_connection(conn,engine,expected).await;});
            }
            _=tokio::time::sleep(Duration::from_millis(200))=>{if engine.lock().await.shutdown{break}}
            _=connections.join_next(), if !connections.is_empty()=>{}
        }
    }
    ticking.abort();
    let _ = ticking.await;
    // Finish the shutdown acknowledgement and any bounded local reads before
    // releasing the database and then the single-worker lock.
    while connections.join_next().await.is_some() {}
    drop(engine);
    drop(listener);
    drop(lock);
    Ok(())
}
async fn handle_connection(
    conn: Stream,
    engine: Arc<Mutex<Engine>>,
    expected: String,
) -> Result<()> {
    verify_peer(&conn)?;
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
fn verify_peer(conn: &Stream) -> Result<()> {
    use interprocess::local_socket::traits::StreamCommon;
    let peer = conn.peer_creds()?;
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and does not change process state.
        if peer.euid() != Some(unsafe { libc::geteuid() }) {
            bail!("Local worker belongs to a different user");
        }
    }
    #[cfg(windows)]
    {
        let pid = peer
            .pid()
            .ok_or_else(|| anyhow!("Local worker identity unavailable"))?;
        if !windows_peer::same_user(pid)? {
            bail!("Local worker belongs to a different user");
        }
    }
    Ok(())
}
#[cfg(windows)]
mod windows_peer {
    use anyhow::{Result, bail};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    pub(super) fn spawn_worker(binary: &std::path::Path, demo: bool) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
            STARTUPINFOW,
        };
        let executable: Vec<u16> = binary.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut arguments: Vec<u16> = format!(
            "\"{}\" {}worker",
            binary.display(),
            if demo { "--demo " } else { "" }
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        // SAFETY: terminated buffers live through the call. No handles are
        // inherited, including a host's redirected CLI/MCP pipes. Windows
        // inherits this process' environment and working directory.
        unsafe {
            let mut startup: STARTUPINFOW = std::mem::zeroed();
            startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(
                executable.as_ptr(),
                arguments.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
                std::ptr::null(),
                std::ptr::null(),
                &startup,
                &mut process,
            ) == 0
            {
                bail!(
                    "Could not launch the bundled worker. Ensure hrctl is installed beside the GUI."
                );
            }
            drop(OwnedHandle::from_raw_handle(process.hThread));
            drop(OwnedHandle::from_raw_handle(process.hProcess));
        }
        Ok(())
    }
    fn token_user(process: HANDLE) -> Result<Vec<usize>> {
        // SAFETY: token handles are owned until the API calls finish; the token
        // buffer uses pointer alignment and the size supplied by Windows.
        unsafe {
            let mut raw = std::ptr::null_mut();
            if OpenProcessToken(process, TOKEN_QUERY, &mut raw) == 0 {
                bail!("Local process identity could not be checked");
            }
            let token = OwnedHandle::from_raw_handle(raw);
            let mut length = 0;
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut length,
            );
            if length == 0 || length > 65536 {
                bail!("Local process identity unavailable");
            }
            let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            ) == 0
            {
                bail!("Local process identity could not be read");
            }
            Ok(buffer)
        }
    }
    pub(super) fn same_user(pid: u32) -> Result<bool> {
        // SAFETY: the peer process handle and both token buffers remain live
        // while EqualSid reads the SIDs returned by GetTokenInformation.
        unsafe {
            let raw = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if raw.is_null() {
                bail!("Local worker identity could not be opened");
            }
            let process = OwnedHandle::from_raw_handle(raw);
            let peer = token_user(process.as_raw_handle())?;
            let current = token_user(GetCurrentProcess())?;
            let peer = &*peer.as_ptr().cast::<TOKEN_USER>();
            let current = &*current.as_ptr().cast::<TOKEN_USER>();
            Ok(EqualSid(peer.User.Sid, current.User.Sid) != 0)
        }
    }
}
fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (left, right) in a.iter().zip(b) {
        diff |= (left ^ right) as usize;
    }
    diff == 0
}
pub fn blocking_request(demo: bool, command: Command) -> Result<Value> {
    // Native clients reuse their I/O driver. Named-pipe operations must not
    // outlive a temporary runtime that is destroyed after each GUI request.
    static RUNTIME: std::sync::OnceLock<Result<tokio::runtime::Runtime, ()>> =
        std::sync::OnceLock::new();
    let runtime = RUNTIME
        .get_or_init(|| tokio::runtime::Runtime::new().map_err(|_| ()))
        .as_ref()
        .map_err(|_| anyhow!("Local control runtime could not start"))?;
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
