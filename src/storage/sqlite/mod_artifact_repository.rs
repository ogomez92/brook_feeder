use rusqlite::Row;

use crate::domain::{ArtifactStatus, ModArtifact, ModArtifactRecord};
use crate::errors::FeederResult;
use crate::storage::sqlite::SqliteStorage;
use crate::storage::traits::ModArtifactRepository;

pub struct SqliteModArtifactRepository {
    storage: SqliteStorage,
}

impl SqliteModArtifactRepository {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

const COLUMNS: &str = "cache_key, kind, source_id, game_id, label, version, channel, \
                       url, sha256, local_path, status, seen_at";

fn row_to_record(row: &Row) -> rusqlite::Result<ModArtifactRecord> {
    let status: String = row.get(10)?;
    Ok(ModArtifactRecord {
        cache_key: row.get(0)?,
        kind: row.get(1)?,
        source_id: row.get(2)?,
        game_id: row.get(3)?,
        label: row.get(4)?,
        version: row.get(5)?,
        channel: row.get(6)?,
        url: row.get(7)?,
        sha256: row.get(8)?,
        local_path: row.get(9)?,
        status: ArtifactStatus::from_str(&status),
        seen_at: row.get(11)?,
    })
}

impl ModArtifactRepository for SqliteModArtifactRepository {
    fn get(&self, cache_key: &str) -> FeederResult<Option<ModArtifactRecord>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM mod_artifacts WHERE cache_key = ?1",
            COLUMNS
        ))?;
        let mut rows = stmt.query_map([cache_key], row_to_record)?;

        match rows.next() {
            Some(record) => Ok(Some(record?)),
            None => Ok(None),
        }
    }

    /// Upsert on `cache_key`. A retried download promotes a `pending` row to
    /// `mirrored` in place, without changing `seen_at` — that timestamp records
    /// when the artifact was first announced, not when it last succeeded.
    fn record(
        &self,
        artifact: &ModArtifact,
        status: ArtifactStatus,
        local_path: Option<&str>,
    ) -> FeederResult<()> {
        let conn = self.storage.connection()?;
        conn.execute(
            "INSERT INTO mod_artifacts
                 (cache_key, kind, source_id, game_id, label, version, channel,
                  url, sha256, local_path, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(cache_key) DO UPDATE SET
                 label = excluded.label,
                 url = excluded.url,
                 sha256 = excluded.sha256,
                 local_path = COALESCE(excluded.local_path, mod_artifacts.local_path),
                 status = excluded.status",
            rusqlite::params![
                artifact.cache_key(),
                artifact.kind.as_str(),
                artifact.source_id,
                artifact.game_id,
                artifact.label,
                artifact.version,
                artifact.channel,
                artifact.url,
                artifact.sha256,
                local_path,
                status.as_str(),
            ],
        )?;
        Ok(())
    }

    fn get_all(&self) -> FeederResult<Vec<ModArtifactRecord>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM mod_artifacts ORDER BY kind, source_id, game_id, version",
            COLUMNS
        ))?;
        let records = stmt
            .query_map([], row_to_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ArtifactKind;

    fn artifact() -> ModArtifact {
        ModArtifact {
            kind: ArtifactKind::ModPackage,
            source_id: "amethyst".to_string(),
            game_id: "dsts".to_string(),
            label: "Time Stranger Access".to_string(),
            version: "1.0-beta04".to_string(),
            channel: "beta".to_string(),
            url: Some("https://example.com/a.zip".to_string()),
            sha256: Some("abc".to_string()),
            gated: false,
            page_url: None,
        }
    }

    fn repo() -> SqliteModArtifactRepository {
        SqliteModArtifactRepository::new(SqliteStorage::in_memory().unwrap())
    }

    #[test]
    fn records_and_reads_back() {
        let repo = repo();
        let a = artifact();
        assert!(repo.get(&a.cache_key()).unwrap().is_none());

        repo.record(&a, ArtifactStatus::Mirrored, Some("plugins/amethyst/dsts/1.0-beta04/a.zip"))
            .unwrap();

        let found = repo.get(&a.cache_key()).unwrap().unwrap();
        assert_eq!(found.status, ArtifactStatus::Mirrored);
        assert_eq!(found.label, "Time Stranger Access");
        assert_eq!(
            found.local_path.as_deref(),
            Some("plugins/amethyst/dsts/1.0-beta04/a.zip")
        );
        assert!(!found.seen_at.is_empty());
    }

    #[test]
    fn retry_promotes_pending_to_mirrored() {
        let repo = repo();
        let a = artifact();

        repo.record(&a, ArtifactStatus::Pending, None).unwrap();
        assert_eq!(
            repo.get(&a.cache_key()).unwrap().unwrap().status,
            ArtifactStatus::Pending
        );

        repo.record(&a, ArtifactStatus::Mirrored, Some("some/path.zip"))
            .unwrap();

        let found = repo.get(&a.cache_key()).unwrap().unwrap();
        assert_eq!(found.status, ArtifactStatus::Mirrored);
        assert_eq!(found.local_path.as_deref(), Some("some/path.zip"));
        assert_eq!(repo.get_all().unwrap().len(), 1, "upsert must not duplicate");
    }

    #[test]
    fn a_known_local_path_survives_a_later_record_without_one() {
        let repo = repo();
        let a = artifact();

        repo.record(&a, ArtifactStatus::Mirrored, Some("some/path.zip"))
            .unwrap();
        repo.record(&a, ArtifactStatus::Mirrored, None).unwrap();

        assert_eq!(
            repo.get(&a.cache_key()).unwrap().unwrap().local_path.as_deref(),
            Some("some/path.zip")
        );
    }

    #[test]
    fn gated_artifacts_are_stored_without_a_path() {
        let repo = repo();
        let mut a = artifact();
        a.gated = true;
        a.version = "1.0-beta05".to_string();

        repo.record(&a, ArtifactStatus::Gated, None).unwrap();

        let found = repo.get(&a.cache_key()).unwrap().unwrap();
        assert_eq!(found.status, ArtifactStatus::Gated);
        assert!(found.local_path.is_none());
    }
}
