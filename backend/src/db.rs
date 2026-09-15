use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

pub async fn init_db(data_dir: &Path) -> Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("myproxy.db");
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db_path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(20)
        .connect_with(options)
        .await?;
    migrate(&pool).await?;
    seed(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<()> {
    let stmts = [
        r#"CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS detection_policies (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            check_interval INTEGER NOT NULL DEFAULT 300,
            check_url TEXT NOT NULL DEFAULT 'https://api.ipify.org',
            timeout_ms INTEGER NOT NULL DEFAULT 10000,
            on_fail TEXT NOT NULL DEFAULT 'temp_isolate',
            isolate_seconds INTEGER NOT NULL DEFAULT 600,
            backoff_base_seconds INTEGER NOT NULL DEFAULT 30,
            max_fails INTEGER NOT NULL DEFAULT 3,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS node_pools (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            source_type TEXT NOT NULL,
            subscription_url TEXT,
            data_format TEXT NOT NULL DEFAULT 'txt',
            encoding_format TEXT NOT NULL DEFAULT 'auto',
            format_config TEXT,
            charset TEXT NOT NULL DEFAULT 'utf-8',
            default_protocol TEXT NOT NULL DEFAULT 'http',
            update_interval INTEGER NOT NULL DEFAULT 3600,
            detection_policy_id TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            last_sync TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS proxy_nodes (
            id TEXT PRIMARY KEY,
            pool_id TEXT NOT NULL,
            protocol TEXT NOT NULL DEFAULT 'http',
            host TEXT NOT NULL,
            port INTEGER NOT NULL,
            username TEXT,
            password TEXT,
            raw TEXT,
            exit_ip TEXT,
            country TEXT,
            country_code TEXT,
            asn INTEGER,
            asn_org TEXT,
            is_residential INTEGER DEFAULT 0,
            latency_ms INTEGER,
            fail_count INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending',
            isolated_until TEXT,
            last_check TEXT,
            last_used TEXT,
            next_check TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(pool_id, protocol, host, port, username)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS service_nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            listen_host TEXT NOT NULL DEFAULT '0.0.0.0',
            listen_port INTEGER NOT NULL,
            enable_http INTEGER NOT NULL DEFAULT 1,
            enable_socks5 INTEGER NOT NULL DEFAULT 1,
            enable_socks5h INTEGER NOT NULL DEFAULT 1,
            username TEXT NOT NULL,
            password TEXT NOT NULL,
            pool_ids TEXT NOT NULL DEFAULT '[]',
            filter_countries TEXT NOT NULL DEFAULT '[]',
            filter_asns TEXT NOT NULL DEFAULT '[]',
            filter_residential INTEGER NOT NULL DEFAULT 0,
            selection_strategy TEXT NOT NULL DEFAULT 'round_robin',
            sticky_ttl INTEGER NOT NULL DEFAULT 300,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS traffic_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            service_id TEXT NOT NULL,
            client_ip TEXT NOT NULL,
            proxy_ip TEXT NOT NULL DEFAULT '',
            dest TEXT NOT NULL DEFAULT '',
            protocol TEXT NOT NULL DEFAULT '',
            bytes_up INTEGER NOT NULL DEFAULT 0,
            bytes_down INTEGER NOT NULL DEFAULT 0,
            ts INTEGER NOT NULL
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_traffic_ts ON traffic_logs(ts)",
        "CREATE INDEX IF NOT EXISTS idx_traffic_client ON traffic_logs(client_ip)",
        "CREATE INDEX IF NOT EXISTS idx_nodes_pool ON proxy_nodes(pool_id)",
        "CREATE INDEX IF NOT EXISTS idx_nodes_status ON proxy_nodes(status)",
    ];
    for stmt in stmts {
        sqlx::query(stmt).execute(pool).await?;
    }
    ensure_column(
        pool,
        "traffic_logs",
        "proxy_ip",
        "ALTER TABLE traffic_logs ADD COLUMN proxy_ip TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    ensure_column(
        pool,
        "traffic_logs",
        "dest",
        "ALTER TABLE traffic_logs ADD COLUMN dest TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    ensure_column(
        pool,
        "traffic_logs",
        "protocol",
        "ALTER TABLE traffic_logs ADD COLUMN protocol TEXT NOT NULL DEFAULT ''",
    )
    .await?;
    Ok(())
}

async fn ensure_column(pool: &SqlitePool, table: &str, column: &str, alter: &str) -> Result<()> {
    let sql = format!("SELECT name FROM pragma_table_info('{table}')");
    let names: Vec<String> = sqlx::query_scalar(&sql).fetch_all(pool).await?;
    if !names.iter().any(|n| n == column) {
        sqlx::query(alter).execute(pool).await?;
    }
    Ok(())
}

async fn seed(pool: &SqlitePool) -> Result<()> {
    let now = now_rfc3339();
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM detection_policies LIMIT 1")
            .fetch_optional(pool)
            .await?;
    if exists.is_none() {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO detection_policies
            (id,name,check_interval,check_url,timeout_ms,on_fail,isolate_seconds,backoff_base_seconds,max_fails,created_at,updated_at)
            VALUES (?,?,300,'https://api.ipify.org',10000,'temp_isolate',600,30,3,?,?)"#,
        )
        .bind(&id)
        .bind("默认检测策略")
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;
    }

    let pwd: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key='admin_password'")
            .fetch_optional(pool)
            .await?;
    if pwd.is_none() {
        let hash = bcrypt::hash("admin", 10)?;
        sqlx::query("INSERT INTO settings(key,value) VALUES('admin_password',?)")
            .bind(hash)
            .execute(pool)
            .await?;
    }

    let host: Option<(String,)> =
        sqlx::query_as("SELECT value FROM settings WHERE key='server_host'")
            .fetch_optional(pool)
            .await?;
    if host.is_none() {
        sqlx::query("INSERT INTO settings(key,value) VALUES('server_host','127.0.0.1')")
            .execute(pool)
            .await?;
    }

    let jwt: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key='jwt_secret'")
        .fetch_optional(pool)
        .await?;
    if jwt.is_none() {
        sqlx::query("INSERT INTO settings(key,value) VALUES('jwt_secret',?)")
            .bind(uuid::Uuid::new_v4().to_string())
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}
