use crate::domain::TrackedRepo;
use crate::errors::{FeederError, FeederResult};
use crate::storage::sqlite::SqliteStorage;
use crate::storage::traits::RepoRepository;

pub struct SqliteRepoRepository {
    storage: SqliteStorage,
}

impl SqliteRepoRepository {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }
}

impl RepoRepository for SqliteRepoRepository {
    fn add(&self, repo: &TrackedRepo) -> FeederResult<i64> {
        let conn = self.storage.connection()?;

        // Reject duplicates (case-insensitive owner/name) before inserting.
        let mut stmt = conn.prepare(
            "SELECT EXISTS(SELECT 1 FROM tracked_repos WHERE owner = ?1 COLLATE NOCASE AND name = ?2 COLLATE NOCASE)",
        )?;
        let exists: bool = stmt.query_row((&repo.owner, &repo.name), |row| row.get(0))?;
        drop(stmt);

        if exists {
            return Err(FeederError::FeedAlreadyExists(repo.full_name()));
        }

        conn.execute(
            "INSERT INTO tracked_repos (owner, name, url) VALUES (?1, ?2, ?3)",
            (&repo.owner, &repo.name, &repo.url),
        )?;

        Ok(conn.last_insert_rowid())
    }

    fn remove(&self, id: i64) -> FeederResult<()> {
        let conn = self.storage.connection()?;
        conn.execute("DELETE FROM tracked_repos WHERE id = ?1", [id])?;
        Ok(())
    }

    fn get_all(&self) -> FeederResult<Vec<TrackedRepo>> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, owner, name, url, added_at FROM tracked_repos ORDER BY owner COLLATE NOCASE, name COLLATE NOCASE",
        )?;

        let repos = stmt.query_map([], |row| {
            Ok(TrackedRepo {
                id: Some(row.get(0)?),
                owner: row.get(1)?,
                name: row.get(2)?,
                url: row.get(3)?,
                added_at: row.get(4)?,
            })
        })?;

        repos.collect::<Result<Vec<_>, _>>().map_err(FeederError::from)
    }

    fn exists(&self, owner: &str, name: &str) -> FeederResult<bool> {
        let conn = self.storage.connection()?;
        let mut stmt = conn.prepare(
            "SELECT EXISTS(SELECT 1 FROM tracked_repos WHERE owner = ?1 COLLATE NOCASE AND name = ?2 COLLATE NOCASE)",
        )?;
        let exists: bool = stmt.query_row((owner, name), |row| row.get(0))?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteStorage;

    fn setup() -> SqliteRepoRepository {
        SqliteRepoRepository::new(SqliteStorage::in_memory().unwrap())
    }

    fn repo(owner: &str, name: &str) -> TrackedRepo {
        TrackedRepo::new(
            owner.to_string(),
            name.to_string(),
            format!("https://github.com/{}/{}", owner, name),
        )
    }

    #[test]
    fn test_add_and_list() {
        let r = setup();
        let id = r.add(&repo("sveltejs", "kit")).unwrap();
        assert!(id > 0);

        let all = r.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].full_name(), "sveltejs/kit");
    }

    #[test]
    fn test_duplicate_case_insensitive() {
        let r = setup();
        r.add(&repo("SvelteJS", "Kit")).unwrap();
        assert!(r.exists("sveltejs", "kit").unwrap());
        let result = r.add(&repo("sveltejs", "kit"));
        assert!(matches!(result, Err(FeederError::FeedAlreadyExists(_))));
    }

    #[test]
    fn test_remove() {
        let r = setup();
        let id = r.add(&repo("a", "b")).unwrap();
        r.remove(id).unwrap();
        assert!(r.get_all().unwrap().is_empty());
    }
}
