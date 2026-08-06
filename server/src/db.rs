use anyhow::Result;
use include_dir::{include_dir, Dir};
use lazy_static::lazy_static;
use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags};
use rusqlite_migration::Migrations;
use std::path::PathBuf;
use std::time::Duration;

static MIGRATIONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/migrations");
const DEFAULT_POOL_MAX_SIZE: u32 = 16;

lazy_static! {
    static ref MIGRATIONS: Migrations<'static> =
        Migrations::from_directory(&MIGRATIONS_DIR).unwrap();
}

#[derive(Debug)]
struct SqliteConnectionCustomizer;

impl CustomizeConnection<Connection, rusqlite::Error> for SqliteConnectionCustomizer {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), rusqlite::Error> {
        set_connection_pragmas(conn)
    }
}

pub fn init_db(path: &PathBuf) -> Result<Pool<SqliteConnectionManager>> {
    let mut conn = Connection::open(path)?;
    set_pragma(&mut conn)?;
    log_sqlite_configuration(&conn)?;
    migrate(&mut conn)?;
    conn.execute_batch("PRAGMA optimize=0x10002;")?;
    drop(conn);

    let manager = SqliteConnectionManager::file(path);
    let pool_max_size = parse_pool_max_size(std::env::var("LRCLIB_POOL_MAX_SIZE").ok().as_deref());
    let pool = r2d2::Pool::builder()
        .connection_timeout(Duration::from_secs(5))
        .max_size(pool_max_size)
        .connection_customizer(Box::new(SqliteConnectionCustomizer))
        .build(manager)?;

    Ok(pool)
}

fn parse_pool_max_size(value: Option<&str>) -> u32 {
    value
        .and_then(|value| value.parse().ok())
        .filter(|&size| size > 0)
        .unwrap_or(DEFAULT_POOL_MAX_SIZE)
}

pub fn open_db_read_only(path: &PathBuf) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

pub fn set_pragma(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    set_connection_pragmas(conn)?;

    Ok(())
}

fn set_connection_pragmas(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;

    let mmap_size = std::env::var("LRCLIB_MMAP_SIZE")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(2000000000i64)
        .to_string();
    conn.pragma_update(None, "mmap_size", &mmap_size)?;

    let cache_size = std::env::var("LRCLIB_CACHE_SIZE")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(-262144i64)
        .to_string();
    conn.pragma_update(None, "cache_size", &cache_size)?;

    Ok(())
}

fn log_sqlite_configuration(conn: &Connection) -> rusqlite::Result<()> {
    let sqlite_version: String = conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let mmap_size: i64 = conn.query_row("PRAGMA mmap_size", [], |row| row.get(0))?;
    let mut statement = conn.prepare("PRAGMA compile_options")?;
    let compile_options = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let max_mmap_size = compile_options
        .iter()
        .find_map(|option| option.strip_prefix("MAX_MMAP_SIZE="))
        .unwrap_or("unknown");

    tracing::info!(
        sqlite_version = %sqlite_version,
        mmap_size_bytes = mmap_size,
        max_mmap_size_bytes = %max_mmap_size,
        "SQLite configuration"
    );
    tracing::debug!(?compile_options, "SQLite compile options");

    Ok(())
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    MIGRATIONS.to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn pool_max_size_uses_positive_environment_values() {
        assert_eq!(parse_pool_max_size(Some("32")), 32);
        assert_eq!(parse_pool_max_size(None), DEFAULT_POOL_MAX_SIZE);
        assert_eq!(parse_pool_max_size(Some("invalid")), DEFAULT_POOL_MAX_SIZE);
        assert_eq!(parse_pool_max_size(Some("0")), DEFAULT_POOL_MAX_SIZE);
    }

    #[test]
    fn read_only_open_does_not_apply_migrations() {
        let path =
            std::env::temp_dir().join(format!("lrclib-read-only-{}.sqlite3", Uuid::new_v4()));
        drop(Connection::open(&path).unwrap());

        let connection = open_db_read_only(&path).unwrap();
        let lyrics_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'lyrics'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lyrics_tables, 0);
        assert!(connection
            .execute("CREATE TABLE should_fail (id INTEGER)", [])
            .is_err());

        drop(connection);
        let _ = std::fs::remove_file(path);
    }
}
