use anyhow::{anyhow, Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// Exclusive process lock for maintenance commands that write to the same
/// database. The lock file lives next to the database and is keyed by a
/// command-specific suffix so maintenance invocations can make their mutual
/// exclusion explicit.
pub struct BackfillLock {
    _file: File,
}

impl BackfillLock {
    pub fn acquire(database: &Path, suffix: &str, label: &str) -> Result<Self> {
        let database = std::fs::canonicalize(database)
            .with_context(|| format!("failed to resolve database path {}", database.display()))?;
        let mut lock_path = database.as_os_str().to_os_string();
        lock_path.push(suffix);
        let lock_path = PathBuf::from(lock_path);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("failed to open backfill lock {}", lock_path.display()))?;
        file.try_lock()
            .map_err(|error| anyhow!("another {label} holds {}: {error}", lock_path.display()))?;
        file.set_len(0)?;
        writeln!(file, "pid={}", std::process::id())?;

        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_database_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lrclib-backfill-lock-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn cleanup(database: &Path, suffix: &str) {
        for path in [
            database.to_path_buf(),
            PathBuf::from(format!("{}-wal", database.display())),
            PathBuf::from(format!("{}-shm", database.display())),
            PathBuf::from(format!("{}{}", database.display(), suffix)),
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn exclusive_lock_rejects_a_second_backfill() {
        let database = test_database_path();
        File::create(&database).unwrap();

        let first =
            BackfillLock::acquire(&database, ".test-backfill.lock", "test backfill").unwrap();
        assert!(BackfillLock::acquire(&database, ".test-backfill.lock", "test backfill").is_err());
        drop(first);
        assert!(BackfillLock::acquire(&database, ".test-backfill.lock", "test backfill").is_ok());

        cleanup(&database, ".test-backfill.lock");
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_lock_uses_canonical_database_identity() {
        use std::os::unix::fs::symlink;

        let database = test_database_path();
        let alias = database.with_extension("alias.sqlite3");
        File::create(&database).unwrap();
        symlink(&database, &alias).unwrap();

        let first =
            BackfillLock::acquire(&database, ".test-backfill.lock", "test backfill").unwrap();
        assert!(BackfillLock::acquire(&alias, ".test-backfill.lock", "test backfill").is_err());
        drop(first);

        let _ = std::fs::remove_file(alias);
        cleanup(&database, ".test-backfill.lock");
    }
}
