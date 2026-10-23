use crate::{model::*, vault::Vault};
use anyhow::{Result, anyhow};
use rusqlite::{Connection as Db, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub struct Store {
    pub db: Db,
    pub vault: Vault,
    memory: HashMap<String, Vec<u8>>,
    pub root: PathBuf,
}
pub fn data_root(demo: bool) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HUMBLE_REDEEMER_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    let dirs = directories::ProjectDirs::from(
        "io.github",
        "brk275",
        if demo {
            "HumbleRedeemerDemo"
        } else {
            "HumbleRedeemer"
        },
    )
    .ok_or_else(|| anyhow!("User application storage unavailable"))?;
    Ok(dirs.data_local_dir().to_path_buf())
}
pub fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
impl Store {
    pub fn open(root: PathBuf, demo: bool) -> Result<Self> {
        private_dir(&root)?;
        let db = Db::open(root.join("state.sqlite"))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS games (id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS secrets (id TEXT PRIMARY KEY, ciphertext BLOB NOT NULL);
          CREATE TABLE IF NOT EXISTS choices (id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS batches (id TEXT PRIMARY KEY, metadata TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS settings (id TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        Ok(Self {
            db,
            vault: Vault::open(demo),
            memory: HashMap::new(),
            root,
        })
    }
    pub fn put_game(&self, g: &Game) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO games VALUES (?1,?2)",
            params![g.id, serde_json::to_string(g)?],
        )?;
        Ok(())
    }
    pub fn games(&self) -> Result<Vec<Game>> {
        self.list("games")
    }
    pub fn game(&self, id: &str) -> Result<Game> {
        self.lookup("games", id)?
            .ok_or_else(|| anyhow!("Game not found"))
    }
    pub fn choices(&self) -> Result<Vec<Choice>> {
        self.list("choices")
    }
    pub fn put_choice(&self, c: &Choice) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO choices VALUES (?1,?2)",
            params![c.id, serde_json::to_string(c)?],
        )?;
        Ok(())
    }
    pub fn clear_choices(&self) -> Result<()> {
        self.db.execute("DELETE FROM choices", [])?;
        Ok(())
    }
    pub fn batches(&self) -> Result<Vec<Batch>> {
        self.list("batches")
    }
    pub fn batch(&self, id: &str) -> Result<Batch> {
        self.lookup("batches", id)?
            .ok_or_else(|| anyhow!("Batch not found"))
    }
    fn lookup<T: DeserializeOwned>(&self, table: &str, id: &str) -> Result<Option<T>> {
        let metadata: Option<String> = self
            .db
            .query_row(
                &format!("SELECT metadata FROM {table} WHERE id=?1"),
                [id],
                |row| row.get(0),
            )
            .optional()?;
        metadata
            .map(|json| Ok(serde_json::from_str(&json)?))
            .transpose()
    }
    pub fn put_batch(&self, b: &Batch) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO batches VALUES (?1,?2)",
            params![b.id, serde_json::to_string(b)?],
        )?;
        Ok(())
    }
    fn list<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let mut stmt = self
            .db
            .prepare(&format!("SELECT metadata FROM {table} ORDER BY rowid"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn secret<T: DeserializeOwned>(&self, id: &str) -> Result<Option<T>> {
        if let Some(plain) = self.memory.get(id) {
            return Ok(Some(serde_json::from_slice(plain)?));
        }
        let sealed: Option<Vec<u8>> = self
            .db
            .query_row("SELECT ciphertext FROM secrets WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        sealed
            .map(|bytes| {
                let plain = self.vault.unseal(&bytes)?;
                Ok(serde_json::from_slice(&plain)?)
            })
            .transpose()
    }
    pub fn put_secret<T: Serialize>(&mut self, id: &str, value: &T) -> Result<()> {
        let plain = zeroize::Zeroizing::new(serde_json::to_vec(value)?);
        if self.vault.persistent {
            self.db.execute(
                "INSERT OR REPLACE INTO secrets VALUES (?1,?2)",
                params![id, self.vault.seal(&plain)?],
            )?;
        } else {
            use zeroize::Zeroize;
            if let Some(mut old) = self.memory.insert(id.into(), plain.to_vec()) {
                old.zeroize();
            }
        }
        Ok(())
    }
    pub fn forget(&mut self, id: &str) -> Result<()> {
        use zeroize::Zeroize;
        if let Some(mut value) = self.memory.remove(id) {
            value.zeroize();
        }
        self.db.execute("DELETE FROM secrets WHERE id=?1", [id])?;
        Ok(())
    }
    pub fn get_setting(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM settings WHERE id=?1", [id], |r| r.get(0))
            .optional()?)
    }
    pub fn setting(&self, id: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO settings VALUES (?1,?2)",
            [id, value],
        )?;
        Ok(())
    }
    pub fn recover(&self) -> Result<()> {
        for mut g in self.games()? {
            if g.outcome == Outcome::InFlight {
                g.outcome = Outcome::UnknownOutcome;
                g.reason = outcome_reason(g.outcome).into();
                self.put_game(&g)?;
            }
        }
        for mut b in self.batches()? {
            if b.state == "claiming" {
                b.state = "needs_review".into();
                b.approved = false;
                b.message =
                    "An interrupted Choice claim needs synchronization and a new review.".into();
                self.put_batch(&b)?;
            } else if b.state == "running" {
                b.state = "paused".into();
                b.message =
                    "Recovered after restart. Reconcile uncertain requests before resuming.".into();
                self.put_batch(&b)?;
            }
        }
        Ok(())
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        for value in self.memory.values_mut() {
            value.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_only_secrets_never_reach_database() {
        let t = tempfile::tempdir().unwrap();
        let mut s = Store::open(t.path().into(), true).unwrap();
        s.put_secret("fixture", &"hidden fixture").unwrap();
        let count: i64 =
            s.db.query_row("SELECT count(*) FROM secrets", [], |r| r.get(0))
                .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            s.secret::<String>("fixture").unwrap().unwrap(),
            "hidden fixture"
        );
    }
    #[test]
    fn interrupted_requests_are_not_blindly_retried() {
        let t = tempfile::tempdir().unwrap();
        let s = Store::open(t.path().into(), true).unwrap();
        let g = Game {
            id: "a".into(),
            title: "Fixture".into(),
            app_id: Some(1),
            source: "Fixture".into(),
            outcome: Outcome::InFlight,
            reason: String::new(),
            service_code: None,
            has_key: false,
            updated_at: now(),
        };
        s.put_game(&g).unwrap();
        s.recover().unwrap();
        assert_eq!(s.game("a").unwrap().outcome, Outcome::UnknownOutcome);
    }
}
