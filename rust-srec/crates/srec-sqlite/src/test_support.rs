//! Fixtures for tests on migrated SQLite databases: per-connection SQL
//! observations and migrated database files.

use std::collections::HashMap;
use std::ffi::{CStr, c_int, c_uint, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type Statements = Arc<Mutex<Vec<String>>>;
static TRACES: OnceLock<Mutex<HashMap<usize, Statements>>> = OnceLock::new();
static NEXT_TRACE: AtomicUsize = AtomicUsize::new(1);

unsafe extern "C" {
    fn sqlite3_trace_v2(
        database: *mut c_void,
        mask: c_uint,
        callback: Option<
            unsafe extern "C" fn(c_uint, *mut c_void, *mut c_void, *mut c_void) -> c_int,
        >,
        context: *mut c_void,
    ) -> c_int;
}

unsafe extern "C" fn record_statement(
    _mask: c_uint,
    context: *mut c_void,
    _statement: *mut c_void,
    sql: *mut c_void,
) -> c_int {
    if sql.is_null() {
        return 0;
    }
    let Some(traces) = TRACES.get() else { return 0 };
    let Ok(traces) = traces.lock() else { return 0 };
    let statements = traces.get(&(context as usize)).cloned();
    drop(traces);
    if let Some(statements) = statements
        && let Ok(mut statements) = statements.lock()
    {
        // SAFETY: SQLITE_TRACE_STMT supplies a non-null, NUL-terminated SQL string
        // valid for this callback; the null case was handled above.
        let sql = unsafe { CStr::from_ptr(sql.cast()) }.to_string_lossy();
        statements.push(sql.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    0
}

/// Observe actual executed statements on a fixture with exactly one connection.
/// Callback contexts are numeric registry keys, never pointers to borrowed data;
/// dropping a trace is safe even if a worker is completing a statement or a test panics.
pub struct SqlTrace {
    key: usize,
    statements: Statements,
}

impl SqlTrace {
    pub async fn install(pool: &sqlx::SqlitePool) -> Self {
        assert_eq!(pool.options().get_max_connections(), 1);
        let key = NEXT_TRACE.fetch_add(1, Ordering::Relaxed);
        let statements = Arc::new(Mutex::new(Vec::new()));
        TRACES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(key, statements.clone());
        let trace = Self { key, statements };
        let mut connection = pool.acquire().await.unwrap();
        let mut handle = connection.lock_handle().await.unwrap();
        // SAFETY: SQLx suspends its worker while this valid handle is locked. The callback is
        // process-lifetime code and looks up an owned counter through the registry.
        let result = unsafe {
            sqlite3_trace_v2(
                handle.as_raw_handle().as_ptr().cast(),
                1,
                Some(record_statement),
                key as *mut c_void,
            )
        };
        assert_eq!(result, 0);
        trace
    }

    pub fn statements(&self) -> Vec<String> {
        self.statements.lock().unwrap().clone()
    }
}

impl Drop for SqlTrace {
    fn drop(&mut self) {
        if let Some(traces) = TRACES.get()
            && let Ok(mut traces) = traces.lock()
        {
            traces.remove(&self.key);
        }
    }
}

/// Write a database with every migration applied to a new file at `path`, for
/// fixtures that need a file, such as a pool of several connections. The
/// migrations run on a single-connection in-memory database, so the template
/// [`crate::run_migrations`] keeps for those is reused, and its image is
/// written out as the file. The file is in rollback-journal mode; the pool
/// the caller opens on it switches it to WAL.
///
/// # Panics
///
/// When the database cannot be migrated or the file cannot be written.
pub async fn write_migrated_database(path: &std::path::Path) {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .expect("open an in-memory database");
    crate::run_migrations(&pool)
        .await
        .expect("migrate the in-memory database");
    let mut connection = pool.acquire().await.expect("acquire its connection");
    let image = connection
        .serialize(None)
        .await
        .expect("serialize the migrated database");
    drop(connection);
    pool.close().await;
    std::fs::write(path, &*image).expect("write the migrated database file");
}
