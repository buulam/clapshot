use diesel::{connection::TransactionManager, migration::Migration};
use diesel::prelude::*;
use diesel::r2d2::ConnectionManager;
use diesel::SqliteConnection;
use anyhow::{Context, anyhow};

use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub mod schema;
pub mod models;
pub mod error;

#[cfg(test)]
pub mod tests;

mod custom_ops;

use error::{DBError, DBResult, EmptyDBResult};
use parking_lot::Mutex;

pub type Pool = diesel::r2d2::Pool<ConnectionManager<SqliteConnection>>;
type PooledConnection = Arc<Mutex<r2d2::PooledConnection<ConnectionManager<SqliteConnection>>>>;

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");


/// Convert a diesel result to a DBResult, turning empty result
/// into a DBError::NotFound
fn to_db_res<U>(res: QueryResult<U>) -> DBResult<U> {
    let res = res.optional();
    match res {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(DBError::NotFound()),
        Err(e) => Err(DBError::BackendError(e)),
    }
}

pub struct DB {
    pool: Pool,
    broken_for_test: AtomicBool,
    connected: Arc<Mutex<Option<PooledConnection>>>
}


impl DB {

    /// Connect to SQLite database with an URL (use this for memory databases)
    pub fn connect_db_url( db_url: &str ) -> DBResult<DB> {
        let manager = ConnectionManager::<SqliteConnection>::new(db_url);
        let pool = Pool::builder().max_size(1).build(manager).context("Failed to build DB pool")?;

        let db = DB {
            pool: pool,
            broken_for_test: AtomicBool::new(false),
            connected: Arc::new(Mutex::new(None))
        };

        diesel::sql_query("PRAGMA foreign_keys = ON;")
            .execute(&mut *db.conn()?.lock())
            .context("Failed to enable foreign keys")?;

        Ok(db)
    }

    /// Connect to SQLite database with a file path
    pub fn connect_db_file( db_file: &Path ) -> DBResult<DB> {
        let db_url = format!("sqlite://{}", db_file.to_str().ok_or(anyhow!("Invalid DB file path"))
            .context("Failed to connect DB file")?);
        DB::connect_db_url(&db_url)
    }

    /// Get a connection from the pool
    pub fn conn(&self) ->  DBResult<PooledConnection> {
        // For testing
        if self.broken_for_test.load(std::sync::atomic::Ordering::Relaxed) {
            let bad_pool = Pool::builder().build(ConnectionManager::<SqliteConnection>::new("sqlite:///dev/urandom")).context("Failed to build 'broken' DB pool")?;
            return bad_pool.get()
                .map(|v| Arc::new(Mutex::new(v)))
                .map_err(|e| anyhow!("Failed to get connection from pool: {:?}", e).into());
        };

        // Use cached connection if available (e.g. for transactions)
        if let Some(conn) = self.connected.lock().as_ref() {
            return Ok(conn.clone());
        }

        // Otherwise get a new connection from the pool
        let res = self.pool.get()
            .map(|v| Arc::new(Mutex::new(v)))
            .map_err(|e| DBError::Other(anyhow!("Failed to get connection from pool: {:?}", e)))?;
        self.connected.lock().replace(res.clone());

        Ok(res)
    }

    /// Return list of any pending migrations
    pub fn pending_migration_names(&self) -> DBResult<Vec<String>> {
        Ok(MigrationHarness::pending_migrations(&mut *self.conn()?.lock(), MIGRATIONS)
            .map_err(|e| anyhow!("Failed to get migrations: {:?}", e))?
            .iter().map(|m| m.name().to_string()).collect())
    }

    /// Run a named migration
    pub fn apply_migration(&self, migration_name: &str) -> EmptyDBResult {
        self.conn()?.lock().transaction(|lock| {
            let pending = MigrationHarness::pending_migrations(&mut *lock, MIGRATIONS)
                .map_err(|e| anyhow!("Failed to get migrations: {:?}", e))?;
            let migration = pending.iter().find(|m| m.name().to_string() == migration_name)
                .ok_or_else(|| anyhow!("Migration not found: {}", migration_name))?;

            tracing::info!("Applying migration: {}", migration.name());
            diesel::sql_query("PRAGMA foreign_keys = OFF;").execute(&mut *lock)?;
            MigrationHarness::run_migration(&mut *lock, &**migration)
                .map_err(|e| anyhow!("Failed to apply migration: {:?}", e))?;
            diesel::sql_query("PRAGMA foreign_keys = ON;").execute(&mut *lock)?;
            Ok(())
        })
    }

    /// "Corrupt" the connection for testing so that subsequent queries fail
    pub fn break_db(&self) {
        self.broken_for_test.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

// ---------------- Transactions ----------------

pub fn begin_transaction(conn: &PooledConnection) -> DBResult<()> {
    diesel::r2d2::PoolTransactionManager::begin_transaction(&mut *conn.lock())
        .map_err(|e| anyhow!("Failed to begin transaction: {:?}", e).into())
}

pub fn commit_transaction(conn: &PooledConnection) -> DBResult<()> {
    diesel::r2d2::PoolTransactionManager::commit_transaction(&mut *conn.lock())
        .map_err(|e| anyhow!("Failed to commit transaction: {:?}", e).into())
}

pub fn rollback_transaction(conn: &PooledConnection) -> DBResult<()> {
    diesel::r2d2::PoolTransactionManager::rollback_transaction(&mut *conn.lock())
        .map_err(|e| anyhow!("Failed to rollback transaction: {:?}", e).into())
}

// ---------------- Savepoints ----------------

/// Start a new named savepoint
pub fn create_savepoint(conn: &PooledConnection, name: &str) -> DBResult<()> {
    diesel::RunQueryDsl::execute(diesel::sql_query(&format!("SAVEPOINT {};", name)), &mut *conn.lock())
        .map_err(|e| anyhow!("Failed to create savepoint: {:?}", e).into())
        .map(|_| ())    // discard row count
}

// Release (commit if not explicitly rolled back) a savepoint
pub fn release_savepoint(conn: &PooledConnection, name: &str) -> DBResult<()> {
    diesel::RunQueryDsl::execute(diesel::sql_query(&format!("RELEASE SAVEPOINT {};", name)), &mut *conn.lock())
        .map_err(|e| anyhow!("Failed to release savepoint: {:?}", e).into())
        .map(|_| ())
}

/// Rollback to a savepoint. Remember to release the savepoint afterwards!
pub fn rollback_to_savepoint(conn: &PooledConnection, name: &str) -> DBResult<()> {
    diesel::RunQueryDsl::execute(diesel::sql_query(&format!("ROLLBACK TO SAVEPOINT {};", name)), &mut *conn.lock())
        .map_err(|e| anyhow!("Failed to rollback to savepoint: {:?}", e).into())
        .map(|_| ())
}

// ---------------- Query traits ----------------

pub struct DBPaging {
    pub page_num: u32,
    pub page_size: std::num::NonZeroU32,
}

impl DBPaging {
    pub fn offset(&self) -> i64 {
        (self.page_num * self.page_size.get()) as i64
    }
    pub fn limit(&self) -> i64 {
        self.page_size.get() as i64
    }
}

impl Default for DBPaging {
    fn default() -> Self {
        Self { page_num: 0, page_size: unsafe { std::num::NonZeroU32::new_unchecked(u32::MAX) } }
    }
}


pub trait DbBasicQuery<P, I>: Sized
    where P: std::str::FromStr + Send + Sync + Clone,
          I: Send + Sync,
{
    /// Insert a new object into the database.
    fn insert(db: &DB, item: &I) -> DBResult<Self>;

    /// Insert multiple objects into the database.
    fn insert_many(db: &DB, items: &[I]) -> DBResult<Vec<Self>>;

    /// Get a single object by its primary key.
    /// Returns None if no object with the given ID was found.
    fn get(db: &DB, pk: &P) -> DBResult<Self>;

    /// Get multiple objects by their primary keys.
    fn get_many(db: &DB, ids: &[P]) -> DBResult<Vec<Self>>;

    /// Get all nodes of type Self, with no filtering, paginated.
    fn get_all(db: &DB, pg: DBPaging) -> DBResult<Vec<Self>>;

    /// Update objects, replaces the entire object except for the primary key.
    fn update_many(db: &DB, items: &[Self]) -> DBResult<Vec<Self>>;

    /// Delete a single object from the database.
    fn delete(db: &DB, id: &P) -> DBResult<bool>;

    /// Delete multiple objects from the database.
    fn delete_many(db: &DB, ids: &[P]) -> DBResult<usize>;
}

mod basic_query;
crate::implement_basic_query_traits!(models::Video, models::VideoInsert, videos, String, added_time.desc());
crate::implement_basic_query_traits!(models::Comment, models::CommentInsert, comments, i32, created.desc());
crate::implement_basic_query_traits!(models::Message, models::MessageInsert, messages, i32, created.desc());


pub trait DbQueryByUser: Sized {
    /// Get all objects of type Self that belong to given user.
    fn get_by_user(db: &DB, uid: &str, pg: DBPaging) -> DBResult<Vec<Self>>;
}
crate::implement_query_by_user_traits!(models::Video, videos, added_time.desc());
crate::implement_query_by_user_traits!(models::Comment, comments, created.desc());
crate::implement_query_by_user_traits!(models::Message, messages, created.desc());



pub trait DbQueryByVideo: Sized {
    /// Get all objects of type Self that are linked to given video.
    fn get_by_video(db: &DB, vid: &str, pg: DBPaging) -> DBResult<Vec<Self>>;
}
crate::implement_query_by_video_traits!(models::Comment, comments, video_id, created.desc());
crate::implement_query_by_video_traits!(models::Message, messages, video_id, created.desc());
