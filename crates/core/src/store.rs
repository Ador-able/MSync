use crate::{
    endpoint::Endpoint,
    snapshot::{Item, Snapshot},
};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pair {
    pub id: String,
    pub local: Endpoint,
    pub remote: crate::adb::Binding,
    pub automatic: bool,
    #[serde(default = "default_ignore_patterns")]
    pub ignore_patterns: Vec<String>,
}
pub fn default_ignore_patterns() -> Vec<String> {
    vec![r"^~\$".into()]
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub generation: String,
    pub snapshots: [Snapshot; 2],
    pub completed: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileFailure {
    pub path: String,
    pub reason: String,
}
pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let db = Connection::open(dir.join("state-adb.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS baseline (pair TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS file_failures (pair TEXT NOT NULL, path TEXT NOT NULL, reason TEXT NOT NULL, PRIMARY KEY(pair,path));")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS file_progress (pair TEXT NOT NULL, path TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(pair,path));")?;
        Ok(Self { db })
    }
    pub fn pair(&self) -> Result<Option<Pair>> {
        let s: Option<String> = self
            .db
            .query_row("SELECT value FROM settings WHERE id=1", [], |r| r.get(0))
            .optional()?;
        Ok(s.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub fn save_pair(&self, pair: &Pair) -> Result<()> {
        self.db.execute(
            "INSERT INTO settings VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value",
            [serde_json::to_string(pair)?],
        )?;
        Ok(())
    }
    pub fn baseline(&self, pair: &str) -> Result<Option<Baseline>> {
        let s: Option<String> = self
            .db
            .query_row("SELECT value FROM baseline WHERE pair=?1", [pair], |r| {
                r.get(0)
            })
            .optional()?;
        let mut baseline: Option<Baseline> = s.map(|s| serde_json::from_str(&s)).transpose()?;
        if let Some(baseline) = &mut baseline {
            let mut query = self
                .db
                .prepare("SELECT path,value FROM file_progress WHERE pair=?1")?;
            let rows = query.query_map([pair], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (path, value) = row?;
                let items: [Option<Item>; 2] = serde_json::from_str(&value)?;
                for (side, item) in items.into_iter().enumerate() {
                    if let Some(item) = item {
                        baseline.snapshots[side].insert(path.clone(), item);
                    } else {
                        baseline.snapshots[side].remove(&path);
                    }
                }
            }
        }
        Ok(baseline)
    }
    pub fn checkpoint(&mut self, pair: &str, baseline: &Baseline) -> Result<()> {
        // 逐文件保存已核验结果；未完成路径继续保留旧基线和删除意图。
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO baseline VALUES(?1,?2) ON CONFLICT(pair) DO UPDATE SET value=excluded.value", params![pair, serde_json::to_string(baseline)?])?;
        tx.execute("DELETE FROM file_progress WHERE pair=?1", [pair])?;
        tx.commit()?;
        Ok(())
    }
    pub fn checkpoint_path(
        &self,
        pair: &str,
        path: &str,
        items: &[Option<crate::snapshot::Item>; 2],
    ) -> Result<()> {
        self.db.execute("INSERT INTO file_progress VALUES(?1,?2,?3) ON CONFLICT(pair,path) DO UPDATE SET value=excluded.value", params![pair, path, serde_json::to_string(items)?])?;
        Ok(())
    }
    pub fn failures(&self, pair: &str) -> Result<Vec<FileFailure>> {
        let mut query = self
            .db
            .prepare("SELECT path,reason FROM file_failures WHERE pair=?1 ORDER BY path")?;
        let rows = query.query_map([pair], |r| {
            Ok(FileFailure {
                path: r.get(0)?,
                reason: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn finish(
        &mut self,
        pair: &str,
        baseline: &Baseline,
        failures: &[FileFailure],
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO baseline VALUES(?1,?2) ON CONFLICT(pair) DO UPDATE SET value=excluded.value",params![pair,serde_json::to_string(baseline)?])?;
        tx.execute("DELETE FROM file_failures WHERE pair=?1", [pair])?;
        tx.execute("DELETE FROM file_progress WHERE pair=?1", [pair])?;
        for failure in failures {
            tx.execute(
                "INSERT INTO file_failures VALUES(?1,?2,?3)",
                params![pair, failure.path, failure.reason],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
