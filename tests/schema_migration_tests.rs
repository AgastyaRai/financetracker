use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use pgvector::Vector;
use rust_decimal::Decimal;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};
use sqlx::{Connection, Executor, FromRow, PgPool};
use std::borrow::Cow;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

const MIGRATION_TEST_DATABASE_URL: &str = "MIGRATION_TEST_DATABASE_URL";
const DATABASE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const LEGACY_SCHEMA_VERSION: i64 = 8;

// keep the connection details needed to create and remove this test's separate database
struct DisposableDatabase {
    admin_options: PgConnectOptions,
    database_name: String,
    pool: PgPool,
}

impl DisposableDatabase {
    async fn create() -> Result<Self> {
        // require an explicit test-server URL so migration tests do not use the normal app database
        let admin_url = std::env::var(MIGRATION_TEST_DATABASE_URL).with_context(|| {
            format!(
                "{MIGRATION_TEST_DATABASE_URL} must explicitly name an admin database reserved for disposable migration tests"
            )
        })?;
        let admin_options = PgConnectOptions::from_str(&admin_url)
            .context("invalid MIGRATION_TEST_DATABASE_URL")?;
        // give each run its own database so old test data cannot affect the upgrade checks
        let database_name = format!(
            "financetracker_migration_test_{}",
            Uuid::new_v4().simple()
        );

        // create the empty database using the connection reserved for migration tests
        let mut admin = connect_with_timeout(&admin_options).await?;
        admin
            .execute(format!("CREATE DATABASE \"{database_name}\"").as_str())
            .await
            .with_context(|| format!("failed to create disposable database {database_name}"))?;
        admin.close().await?;

        // connect a small pool to the new database, with a timeout if the server is unavailable
        let test_options = admin_options.clone().database(&database_name);
        let pool = match tokio::time::timeout(
            DATABASE_CONNECT_TIMEOUT,
            PgPoolOptions::new()
                .max_connections(5)
                .connect_with(test_options),
        )
        .await
        {
            Ok(Ok(pool)) => pool,
            Ok(Err(error)) => {
                // remove the database if its pool could not connect, then return the original error
                drop_database(&admin_options, &database_name).await.ok();
                return Err(error).context("failed to connect to disposable migration database");
            }
            Err(error) => {
                drop_database(&admin_options, &database_name).await.ok();
                return Err(error).context("timed out connecting to disposable migration database");
            }
        };

        Ok(Self {
            admin_options,
            database_name,
            pool,
        })
    }

    async fn cleanup(self) -> Result<()> {
        // close the test connections before dropping the database and all of its records
        self.pool.close().await;
        drop_database(&self.admin_options, &self.database_name).await
    }
}

// bound the wait for an admin connection so an unavailable test server does not hang the test
async fn connect_with_timeout(options: &PgConnectOptions) -> Result<PgConnection> {
    tokio::time::timeout(
        DATABASE_CONNECT_TIMEOUT,
        PgConnection::connect_with(options),
    )
    .await
    .context("timed out connecting to the migration-test admin database")?
    .context("failed to connect to the migration-test admin database")
}

// remove the disposable database, including any connections left behind by a failed assertion
async fn drop_database(options: &PgConnectOptions, database_name: &str) -> Result<()> {
    let mut admin = connect_with_timeout(options).await?;
    admin
        .execute(format!("DROP DATABASE \"{database_name}\" WITH (FORCE)").as_str())
        .await
        .with_context(|| format!("failed to drop disposable database {database_name}"))?;
    admin.close().await?;
    Ok(())
}

// load the real migration files so the test exercises the same upgrades as the application
async fn migration_set() -> Result<Migrator> {
    Migrator::new(Path::new("./backend/migrations"))
        .await
        .context("failed to load FinanceTracker migrations")
}

// select only the migrations that existed before bank-import support was added
fn migrations_through(migrator: &Migrator, maximum_version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            migrator
                .iter()
                .filter(|migration| migration.version <= maximum_version)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
}

// always attempt cleanup before returning an assertion error from the upgrade checks
async fn finish_test(database: DisposableDatabase, result: Result<()>) -> Result<()> {
    let cleanup_result = database.cleanup().await;
    result?;
    cleanup_result
}

// insert a user using only fields available in the old schema, with a known creation time
async fn seed_legacy_user(pool: &PgPool, user_id: Uuid, label: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, created_at)
         VALUES ($1, $2, $3, $4, '2025-01-02 03:04:05+00')",
    )
    .bind(user_id)
    .bind(label)
    .bind(format!("{label}@example.com"))
    .bind("legacy-password-hash")
    .execute(pool)
    .await?;

    Ok(())
}

// insert a transaction in the old format so the upgrade has existing financial data to preserve
async fn seed_legacy_transaction(
    pool: &PgPool,
    transaction_id: Uuid,
    user_id: Option<Uuid>,
    amount: Decimal,
    kind: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO transactions
            (id, user_id, amount, kind, category, description, date, created_at)
         VALUES
            ($1, $2, $3, $4, 'Legacy category', 'Legacy description', '2025-02-03',
             '2025-02-04 05:06:07+00')",
    )
    .bind(transaction_id)
    .bind(user_id)
    .bind(amount)
    .bind(kind)
    .execute(pool)
    .await?;

    Ok(())
}

// insert an old-format embedding and return its vector for comparison after the upgrade
async fn seed_legacy_embedding(
    pool: &PgPool,
    transaction_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<f32>> {
    // use a fixed nonzero vector with the 1536 dimensions required by the database
    let mut values = vec![0.0; 1536];
    values[7] = 1.0;

    sqlx::query(
        "INSERT INTO transaction_embeddings
            (transaction_id, user_id, embedding_text, embedding)
         VALUES ($1, $2, 'legacy canonical text', $3)",
    )
    .bind(transaction_id)
    .bind(user_id)
    .bind(Vector::from(values.clone()))
    .execute(pool)
    .await?;

    Ok(values)
}

#[derive(FromRow)]
struct UpgradedLegacyTransaction {
    user_id: Uuid,
    amount: Decimal,
    kind: String,
    category: Option<String>,
    description: Option<String>,
    date: NaiveDate,
    origin: String,
    status: String,
    currency: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    kind_overridden: bool,
    category_overridden: bool,
    description_overridden: bool,
    search_revision: i32,
    row_version: i64,
}

#[derive(FromRow)]
struct UpgradedLegacyEmbedding {
    user_id: Uuid,
    embedding_text: String,
    embedding: Vector,
    search_revision: i32,
    embedding_model: String,
}

// see if upgrading a populated old database preserves its transactions and embeddings
#[tokio::test]
async fn populated_legacy_database_upgrades_without_losing_financial_or_embedding_data() -> Result<()> {
    // run the checks in a separate database and remove it whether they pass or fail
    let database = DisposableDatabase::create().await?;
    let result = verify_populated_legacy_upgrade(&database.pool).await;
    finish_test(database, result).await
}

async fn verify_populated_legacy_upgrade(pool: &PgPool) -> Result<()> {
    // start at migration 008 so we can insert records before the import fields and signed amounts exist
    let all_migrations = migration_set().await?;
    migrations_through(&all_migrations, LEGACY_SCHEMA_VERSION)
        .run(pool)
        .await?;

    // create an old user and income with a large amount that fits the existing two-decimal precision
    let user_id = Uuid::new_v4();
    let transaction_id = Uuid::new_v4();
    let expense_transaction_id = Uuid::new_v4();
    let legacy_amount = Decimal::from_str("1234567890123.45")?;
    seed_legacy_user(pool, user_id, "legacy_owner").await?;
    seed_legacy_transaction(
        pool,
        transaction_id,
        Some(user_id),
        legacy_amount,
        "income",
    )
    .await?;
    // old expenses were stored as positive amounts; the upgrade should convert this to -34.56
    seed_legacy_transaction(
        pool,
        expense_transaction_id,
        Some(user_id),
        Decimal::new(3456, 2),
        "expense",
    )
    .await?;
    // keep the original embedding vector so we can prove the upgrade did not replace it
    let legacy_vector = seed_legacy_embedding(pool, transaction_id, user_id).await?;

    // apply the remaining migrations, then run the migration runner again to check repeat upgrades
    // it should leave the existing records intact and should not flip the expense amount a second time
    all_migrations.run(pool).await?;
    all_migrations.run(pool).await?;

    // an existing user has not selected a reporting currency, so the upgrade should leave it unset
    let reporting_currency = sqlx::query_scalar::<_, Option<String>>(
        "SELECT reporting_currency FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        reporting_currency.is_none(),
        "migration must not invent a reporting currency for an existing user"
    );

    // read the income by its original ID to check its financial fields and the new defaults
    let transaction = sqlx::query_as::<_, UpgradedLegacyTransaction>(
        "SELECT user_id, amount, kind, category, description, date, origin, status, currency,
                created_at, updated_at, kind_overridden, category_overridden,
                description_overridden, search_revision, row_version
         FROM transactions
         WHERE id = $1",
    )
    .bind(transaction_id)
    .fetch_one(pool)
    .await?;

    // preserve the owner, signed income amount, classification, descriptive fields, and transaction date
    ensure!(transaction.user_id == user_id, "legacy transaction owner changed");
    ensure!(transaction.amount == legacy_amount, "legacy income amount changed");
    ensure!(transaction.kind == "income", "legacy kind changed");
    ensure!(transaction.category.as_deref() == Some("Legacy category"), "legacy category changed");
    ensure!(transaction.description.as_deref() == Some("Legacy description"), "legacy description changed");
    ensure!(transaction.date == NaiveDate::from_ymd_opt(2025, 2, 3).unwrap(), "legacy transaction date changed");
    let legacy_created_at = DateTime::parse_from_rfc3339("2025-02-04T05:06:07+00:00")?
        .with_timezone(&Utc);
    ensure!(transaction.created_at == legacy_created_at, "legacy creation time changed");

    // old manually entered transactions should receive the agreed defaults without invented currency or overrides
    ensure!(transaction.origin == "manual", "legacy origin was not manual");
    ensure!(transaction.status == "posted", "legacy status was not posted");
    ensure!(transaction.currency.is_none(), "migration invented legacy currency");
    ensure!(
        transaction.updated_at == transaction.created_at,
        "updated_at must initially preserve the legacy creation time"
    );
    ensure!(
        !transaction.kind_overridden
            && !transaction.category_overridden
            && !transaction.description_overridden,
        "legacy override markers must begin false"
    );
    ensure!(transaction.search_revision == 1, "legacy transaction revision was not 1");
    ensure!(transaction.row_version == 1, "legacy transaction row version was not 1");

    // read the old expense by its original ID and check that its positive amount became an outflow
    let expense = sqlx::query_as::<_, UpgradedLegacyTransaction>(
        "SELECT user_id, amount, kind, category, description, date, origin, status, currency,
                created_at, updated_at, kind_overridden, category_overridden,
                description_overridden, search_revision, row_version
         FROM transactions
         WHERE id = $1",
    )
    .bind(expense_transaction_id)
    .fetch_one(pool)
    .await?;
    ensure!(expense.user_id == user_id, "legacy expense owner changed");
    ensure!(expense.kind == "expense", "legacy expense kind changed");
    ensure!(expense.row_version == 1, "legacy expense row version was not 1");
    ensure!(expense.amount == Decimal::new(-3456, 2), "legacy expense was not converted to a negative outflow");

    // the amount's sign represents money flow; the rejected direction and source-history columns should not exist
    let rejected_column_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND table_name = 'transactions'
           AND column_name IN ('direction', 'current_source_revision_number')",
    )
    .fetch_one(pool)
    .await?;
    ensure!(rejected_column_count == 0, "rejected transaction columns were reintroduced");

    // read the original embedding to check that its owner, text, and vector survived the upgrade
    let embedding = sqlx::query_as::<_, UpgradedLegacyEmbedding>(
        "SELECT user_id, embedding_text, embedding, search_revision, embedding_model
         FROM transaction_embeddings
         WHERE transaction_id = $1",
    )
    .bind(transaction_id)
    .fetch_one(pool)
    .await?;
    ensure!(embedding.user_id == user_id, "legacy embedding owner changed");
    ensure!(embedding.embedding_text == "legacy canonical text", "embedding text changed");
    ensure!(embedding.embedding.to_vec() == legacy_vector, "embedding vector changed");
    // old embeddings have no known generation revision or model, so the migration must mark those as unknown
    ensure!(embedding.search_revision == 0, "legacy embedding must remain unverified");
    ensure!(
        embedding.embedding_model == "legacy:unknown",
        "legacy embedding model provenance was invented"
    );

    // upgrading old manual transactions should not invent bank connections, imported source data, or exchange rates
    for table_name in [
        "bank_connections",
        "imported_transaction_details",
        "exchange_rate_cache",
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table_name}"))
            .fetch_one(pool)
            .await?;
        ensure!(count == 0, "{table_name} must start empty");
    }

    // store a large negative value within NUMERIC(15,2) to check that its sign and cents are preserved
    let signed_amount = -legacy_amount;
    sqlx::query("UPDATE transactions SET amount = $1 WHERE id = $2")
        .bind(signed_amount)
        .bind(transaction_id)
        .execute(pool)
        .await?;
    // read it back to check the actual stored value rather than only whether the update succeeded
    let stored_amount = sqlx::query_scalar::<_, Decimal>(
        "SELECT amount FROM transactions WHERE id = $1",
    )
    .bind(transaction_id)
    .fetch_one(pool)
    .await?;
    ensure!(stored_amount == signed_amount, "signed amount precision was not preserved");

    Ok(())
}
