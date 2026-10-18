use sqlx::{ConnectOptions, PgPool, migrate::Migrator};
use std::{borrow::Cow, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const MIGRATIONS: Migrator = sqlx::migrate!("./backend/migrations");
const TEST_JWT_SECRET: &str = "deployment_upgrade_test_only";

async fn legacy_schema(pool: &PgPool) {
    // Model an existing deployment that applied the migrations before import provenance was added.
    let legacy_migrations = Migrator {
        migrations: Cow::Owned(
            MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 8)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    legacy_migrations.run(pool).await.unwrap();
}

fn backend(
    pool: &PgPool,
    run_migrations: bool,
    port: u16,
    provider_proxy: &str,
    directory: &TestDirectory,
) -> Command {
    // SQLx 0.7 serializes Prefer as PREFERRED, which its URL parser does not accept; retain the same mode with its accepted spelling.
    let database_url = pool
        .connect_options()
        .to_url_lossy()
        .to_string()
        .replace("ssl-mode=PREFERRED", "ssl-mode=prefer");
    let mut command = Command::new(env!("CARGO_BIN_EXE_financetracker"));
    command
        .env("DATABASE_URL", database_url)
        .env("JWT_SECRET", TEST_JWT_SECRET)
        .env("OPENAI_API_KEY", "no_provider_requests_in_this_test")
        .env("PORT", port.to_string())
        .env_remove("RUN_MIGRATIONS")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // send this child's openai requests through the rejecting test proxy, even if the caller allows a bypass.
    isolate_provider_requests(&mut command, provider_proxy, directory);
    if run_migrations {
        command.env("RUN_MIGRATIONS", "1");
    }
    command
}

fn isolate_provider_requests(command: &mut Command, provider_proxy: &str, directory: &TestDirectory) {
    // remove REQUEST_METHOD because it makes the HTTP client bypass environment proxies.
    // use an empty backend/.env in the child's directory so dotenv cannot restore REQUEST_METHOD.
    command
        .env_remove("REQUEST_METHOD")
        .current_dir(&directory.path);
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(name, provider_proxy);
    }
    for name in ["NO_PROXY", "no_proxy"] {
        command.env(name, "");
    }
}

// run the child until it exits or the deadline fires, and wait for it to stop before returning.
async fn child_output_until(
    command: &mut Command,
    deadline: impl std::future::Future<Output = ()>,
) -> std::io::Result<std::process::Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = tokio::select! {
        output = async {
            // drain both output pipes while waiting so a full pipe cannot block the child from exiting.
            let (status, _, _) = tokio::try_join!(
                child.wait(),
                async {
                    if let Some(pipe) = stdout_pipe.as_mut() {
                        pipe.read_to_end(&mut stdout).await?;
                    }
                    Ok::<_, std::io::Error>(())
                },
                async {
                    if let Some(pipe) = stderr_pipe.as_mut() {
                        pipe.read_to_end(&mut stderr).await?;
                    }
                    Ok::<_, std::io::Error>(())
                },
            )?;
            Ok(std::process::Output {
                status,
                stdout,
                stderr,
            })
        } => output,
        _ = deadline => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "child deadline elapsed",
        )),
    };
    // retain the child across cancellation and await termination before its working directory can be removed.
    child.kill().await?;
    result
}

fn available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// observe the real worker without forwarding its CONNECT request or reading provider credentials.
struct RejectingProviderProxy {
    url: String,
    requests: tokio::sync::mpsc::UnboundedReceiver<String>,
    task: tokio::task::JoinHandle<()>,
}

impl RejectingProviderProxy {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = String::new();
                let read = tokio::time::timeout(Duration::from_secs(1), async {
                    let mut request_reader = BufReader::new((&mut socket).take(8192));
                    request_reader.read_line(&mut request).await
                })
                .await;
                if matches!(read, Ok(Ok(_))) {
                    let _ = socket
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    let request_line = request.trim_end().to_string();
                    let _ = sender.send(request_line);
                }
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

impl Drop for RejectingProviderProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[sqlx::test(migrations = false)]
async fn startup_rejects_missing_import_schema_before_serving_requests(pool: PgPool) {
    legacy_schema(&pool).await;
    let directory = TestDirectory::new();
    let mut provider_proxy = RejectingProviderProxy::start().await;
    let output = child_output_until(
        &mut backend(&pool, false, available_port(), &provider_proxy.url, &directory),
        tokio::time::sleep(Duration::from_secs(5)),
    )
    .await
    .expect("an unmigrated backend must exit instead of serving broken transaction routes");

    assert!(!output.status.success());
    assert!(provider_proxy.requests.try_recv().is_err(), "schema rejection must happen before provider work");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("bank_connections"),
        "missing table should be identified: {error}"
    );
    assert!(
        error.contains("imported_transaction_details"),
        "missing table should be identified: {error}"
    );
    assert!(
        error.contains("RUN_MIGRATIONS=1"),
        "startup should explain how to apply the upgrade: {error}"
    );
    let directory_path = directory.path.clone();
    drop(directory);
    assert!(!directory_path.exists(), "startup fixture directory must be removed");
}

async fn financial_rows(pool: &PgPool, user_id: uuid::Uuid) -> Result<serde_json::Value, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'user', (SELECT to_jsonb(u) FROM users u WHERE id = $1),
            'transactions', (SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM transactions t WHERE user_id = $1),
            'budgets', (SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM budgets b WHERE user_id = $1))"
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

// keep the startup results until the child and its fixture have been cleaned up
struct StartupObservation {
    status: reqwest::StatusCode,
    transactions: serde_json::Value,
    financial_rows: serde_json::Value,
    applied: Vec<i64>,
    provider_request: Option<String>,
}

#[sqlx::test(migrations = false)]
async fn startup_migrations_upgrade_existing_rows_and_are_safe_to_repeat(pool: PgPool) {
    legacy_schema(&pool).await;
    let user_id = uuid::Uuid::new_v4();
    let transaction_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, 'upgrade-test', 'upgrade-test@example.com', 'unused-test-hash')")
        .bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO transactions (id, user_id, amount, kind, category, description, date) VALUES ($1, $2, 25.50, 'expense', 'Groceries', 'Existing transaction', '2026-10-12')")
        .bind(transaction_id).bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO budgets (user_id, month, category, amount) VALUES ($1, '2026-10-01', 'Groceries', 250.00)")
        .bind(user_id).execute(&pool).await.unwrap();
    let mut expected_rows = financial_rows(&pool, user_id).await.unwrap();
    // The import migrations convert the legacy expense sign and add defaults; every other stored field must survive unchanged.
    expected_rows["user"]["reporting_currency"] = serde_json::Value::Null;
    let transaction = &mut expected_rows["transactions"][0];
    transaction["amount"] = serde_json::json!(-25.50);
    transaction["origin"] = serde_json::json!("manual");
    transaction["status"] = serde_json::json!("posted");
    transaction["currency"] = serde_json::Value::Null;
    transaction["kind_overridden"] = serde_json::json!(false);
    transaction["category_overridden"] = serde_json::json!(false);
    transaction["description_overridden"] = serde_json::json!(false);
    transaction["search_revision"] = serde_json::json!(1);
    transaction["row_version"] = serde_json::json!(1);
    transaction["updated_at"] = transaction["created_at"].clone();
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &serde_json::json!({"sub": user_id.to_string(), "exp": chrono::Utc::now().timestamp() + 300}),
        &jsonwebtoken::EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    ).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();

    // Exercise the real binary twice: initial deployment upgrades the schema, and a restart must not reapply it.
    for _ in 0..2 {
        let directory = TestDirectory::new();
        let mut provider_proxy = RejectingProviderProxy::start().await;
        let port = available_port();
        let mut child = backend(&pool, true, port, &provider_proxy.url, &directory)
            .spawn()
            .unwrap();
        let observation: anyhow::Result<StartupObservation> = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let response = loop {
                if let Some(status) = child.try_wait()? {
                    anyhow::bail!("backend exited before serving the upgraded schema: {status}");
                }
                if let Ok(response) = client
                    .get(format!("http://127.0.0.1:{port}/api/transactions"))
                    .bearer_auth(&token)
                    .send()
                    .await
                {
                    break response;
                }
                anyhow::ensure!(
                    tokio::time::Instant::now() < deadline,
                    "upgraded backend did not start"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            let status = response.status();
            let transactions = response.json().await?;
            let financial_rows = financial_rows(&pool, user_id).await?;
            let applied = sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version IN (9, 11) AND success ORDER BY version")
                .fetch_all(&pool)
                .await?;
            let provider_request = tokio::time::timeout(
                Duration::from_secs(5),
                provider_proxy.requests.recv(),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "the real child worker did not reach its rejecting proxy at {}",
                    provider_proxy.url,
                )
            })?;
            Ok(StartupObservation {
                status,
                transactions,
                financial_rows,
                applied,
                provider_request,
            })
        }
        .await;
        // all fallible observations return here so failed checks cannot skip awaiting child cleanup.
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        provider_proxy.task.abort();
        let _ = (&mut provider_proxy.task).await;
        drop(provider_proxy);
        let directory_path = directory.path.clone();
        drop(directory);
        assert!(!directory_path.exists(), "startup fixture directory must be removed");
        let observation = observation.unwrap();
        assert_eq!(observation.status, reqwest::StatusCode::OK);
        let transactions = observation.transactions;
        assert_eq!(transactions.as_array().unwrap().len(), 1);
        assert_eq!(transactions[0]["id"], transaction_id.to_string());
        assert_eq!(transactions[0]["can_revert"], false);
        assert_eq!(observation.financial_rows, expected_rows);
        assert_eq!(observation.applied, vec![9, 11]);
        assert_eq!(
            observation.provider_request.as_deref(),
            Some("CONNECT api.openai.com:443 HTTP/1.1"),
            "the child must attempt indexing through the test's rejecting proxy",
        );
        assert_eq!(financial_rows(&pool, user_id).await.unwrap(), expected_rows);
        let embeddings: i64 = sqlx::query_scalar("SELECT count(*) FROM transaction_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(embeddings, 0, "a rejected provider request must leave indexing unfinished");
    }
}

// a controlled dotenv file prevents the probe from ever reading development credentials.
struct TestDirectory {
    path: std::path::PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let directory = Self {
            path: std::env::temp_dir().join(format!("financetracker-startup-{}", uuid::Uuid::new_v4())),
        };
        std::fs::create_dir(&directory.path).unwrap();
        std::fs::create_dir(directory.path.join("backend")).unwrap();
        std::fs::write(directory.path.join("backend/.env"), "").unwrap();
        directory
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        // remove only the known fixture files and directories; never recursively remove caller data.
        let _ = std::fs::remove_file(self.path.join("backend/.env"));
        let _ = std::fs::remove_dir(self.path.join("backend"));
        let _ = std::fs::remove_dir(&self.path);
    }
}

// run the child with inherited settings, then check that only the rejecting proxy received its request.
async fn assert_loopback_child_isolated(
    inherited_request_method: Option<&str>,
    poison_caller_dotenv: bool,
    serial_child: bool,
) {
    // prepare the caller's dotenv file separately from the empty file the child must load.
    let caller_directory = TestDirectory::new();
    let caller_directory_path = caller_directory.path.clone();
    if poison_caller_dotenv {
        std::fs::write(
            caller_directory.path.join("backend/.env"),
            "REQUEST_METHOD=POST\nF9_DOTENV_POISON=loaded\n",
        )
        .unwrap();
    }
    let mut proxy = RejectingProviderProxy::start().await;
    let proxy_address = reqwest::Url::parse(&proxy.url).unwrap();
    let proxy_address = format!("127.0.0.1:{}", proxy_address.port().unwrap());
    // a bypass can reach only this loopback listener, which closes without a TLS handshake.
    let direct_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let direct_listener_address = direct_listener.local_addr().unwrap();
    let mut direct_listener_task = tokio::spawn(async move {
        direct_listener.accept().await.unwrap();
    });
    // start the child with the caller's settings so isolation must replace them before it runs.
    let child_directory = TestDirectory::new();
    let child_directory_path = child_directory.path.clone();
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--ignored",
            "--exact",
            "deployment_upgrade_tests::test_loopback_provider_child",
            "--nocapture",
        ])
        .current_dir(&caller_directory.path)
        .env("F9_LOOPBACK_TARGET", format!("https://{direct_listener_address}/probe"))
        .env_remove("F9_DOTENV_POISON")
        .kill_on_drop(true);
    match inherited_request_method {
        Some(value) => {
            child.env("REQUEST_METHOD", value);
        }
        None => {
            child.env_remove("REQUEST_METHOD");
        }
    }
    if serial_child {
        child.env("RUST_TEST_THREADS", "1");
    }
    isolate_provider_requests(&mut child, &proxy.url, &child_directory);
    let output = child_output_until(
        &mut child,
        tokio::time::sleep(Duration::from_secs(8)),
    )
    .await;
    let direct_connection = tokio::time::timeout(
        Duration::from_millis(100),
        &mut direct_listener_task,
    )
    .await
    .is_ok();
    if !direct_connection {
        direct_listener_task.abort();
        let _ = direct_listener_task.await;
    }
    // finish stopping the listeners and remove both dotenv directories before checking the results.
    let request = proxy.requests.try_recv().ok();
    proxy.task.abort();
    let _ = (&mut proxy.task).await;
    drop(proxy);
    drop(caller_directory);
    drop(child_directory);
    assert!(
        !caller_directory_path.exists(),
        "caller fixture directory must be removed before assertions"
    );
    assert!(
        !child_directory_path.exists(),
        "child fixture directory must be removed before assertions"
    );
    // socket teardown can finish after task cancellation; require both listeners to close within a deadline.
    for address in [proxy_address, direct_listener_address.to_string()] {
        tokio::time::timeout(Duration::from_secs(5), async {
            while tokio::net::TcpStream::connect(&address).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loopback listeners must close after child and task cleanup");
    }
    let output = output.expect("loopback child must exit within its request deadline");
    assert!(
        output.status.success(),
        "loopback child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    // find the child state after any prefix printed by the serial test harness
    let state_json = stdout
        .lines()
        .find_map(|line| line.split_once("F9_CHILD_STATE=").map(|(_, state)| state))
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(state_json).unwrap();
    println!("F9_LOOPBACK_OBSERVATION={}", serde_json::json!({
        "inherited_request_method": inherited_request_method,
        "dotenv_method": poison_caller_dotenv,
        "direct_connection": direct_connection,
        "proxy_request": request,
        "child": state,
        "directory_removed": true,
        "listeners_closed": true,
    }));
    assert!(
        !direct_connection,
        "child bypassed its rejecting proxy and connected directly to loopback"
    );
    let expected_request = format!("CONNECT {direct_listener_address} HTTP/1.1");
    assert_eq!(request.as_deref(), Some(expected_request.as_str()));
    assert_eq!(
        state["request_method_present"],
        false,
        "REQUEST_METHOD must remain absent after dotenv loading"
    );
    assert_eq!(
        state["dotenv_poison_loaded"],
        false,
        "child must not load the caller's dotenv file"
    );
}

// use the test module
#[cfg(test)]
mod deployment_upgrade_tests {
    use super::*;

    // the test harness is the child entry point, so no test endpoint is added to the production binary.
    #[test]
    #[ignore = "subprocess fixture invoked by the loopback-only isolation regressions"]
    fn test_loopback_provider_child() {
        let target = std::env::var("F9_LOOPBACK_TARGET").unwrap();
        let target = reqwest::Url::parse(&target).unwrap();
        assert_eq!(target.scheme(), "https");
        assert_eq!(target.host_str(), Some("127.0.0.1"));
        dotenvy::from_filename("backend/.env").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let response = reqwest::Client::new()
                .post(target)
                .timeout(Duration::from_secs(2))
                .send()
                .await;
            assert!(response.is_err(), "the loopback endpoints reject all provider requests");
        });
        println!("F9_CHILD_STATE={}", serde_json::json!({
            "request_method_present": std::env::var_os("REQUEST_METHOD").is_some(),
            "dotenv_poison_loaded": std::env::var_os("F9_DOTENV_POISON").is_some(),
        }));
    }

    #[tokio::test]
    async fn test_loopback_child_ignores_inherited_request_method() {
        // inherit both a normal and an empty REQUEST_METHOD without loading a poisoned dotenv file.
        let poison_caller_dotenv = false;
        let serial_child = false;
        for request_method in ["GET", ""] {
            assert_loopback_child_isolated(
                Some(request_method),
                poison_caller_dotenv,
                serial_child,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn test_loopback_child_prevents_dotenv_restoring_request_method() {
        // put REQUEST_METHOD in the caller's dotenv file; the child must load only its own empty file.
        let inherited_request_method = None;
        let poison_caller_dotenv = true;
        let serial_child = false;
        assert_loopback_child_isolated(
            inherited_request_method,
            poison_caller_dotenv,
            serial_child,
        )
        .await;
    }

    #[tokio::test]
    async fn test_loopback_child_uses_proxy_without_request_method() {
        // check the normal case with no inherited REQUEST_METHOD and an empty caller dotenv file.
        let inherited_request_method = None;
        let poison_caller_dotenv = false;
        let serial_child = false;
        assert_loopback_child_isolated(
            inherited_request_method,
            poison_caller_dotenv,
            serial_child,
        )
        .await;
    }

    #[test]
    fn test_startup_directory_is_removed_on_unwind() {
        let directory = TestDirectory::new();
        let path = directory.path.clone();
        let result = std::panic::catch_unwind(move || {
            let _directory = directory;
            panic!("exercise startup-fixture cleanup after a failed assertion");
        });
        assert!(result.is_err());
        assert!(!path.exists(), "startup fixture must also be removed during unwinding");
    }

    #[tokio::test]
    async fn test_serial_child_output_retains_isolation_state() {
        // use one test thread so the harness prints its test name before the child state marker.
        let inherited_request_method = None;
        let poison_caller_dotenv = false;
        let serial_child = true;
        assert_loopback_child_isolated(
            inherited_request_method,
            poison_caller_dotenv,
            serial_child,
        )
        .await;
    }

    #[test]
    #[ignore = "subprocess fixture invoked by the active-child cleanup regression"]
    fn test_held_cleanup_child() {
        let address: std::net::SocketAddr = std::env::var("F9_CLEANUP_READY")
            .unwrap()
            .parse()
            .unwrap();
        assert!(address.ip().is_loopback());
        let _ready = std::net::TcpStream::connect(address).unwrap();
        std::thread::sleep(Duration::from_secs(20));
    }

    #[tokio::test]
    async fn test_active_child_deadline_reaps_before_directory_cleanup() {
        let mut leaked = 0;
        for attempt in 0..12 {
            let directory = TestDirectory::new();
            let path = directory.path.clone();
            let ready = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--ignored",
                    "--exact",
                    "deployment_upgrade_tests::test_held_cleanup_child",
                    "--nocapture",
                ])
                .env("F9_CLEANUP_READY", ready.local_addr().unwrap().to_string())
                .kill_on_drop(true);
            isolate_provider_requests(&mut command, "http://127.0.0.1:9", &directory);
            // wait for the child to confirm it is running before triggering the deadline
            let mut was_active = false;
            let result = child_output_until(&mut command, async {
                was_active = tokio::time::timeout(Duration::from_secs(5), ready.accept())
                    .await
                    .is_ok();
            })
            .await;
            drop(directory);
            let left_behind = path.exists();
            leaked += usize::from(left_behind);
            println!("F9_ACTIVE_CLEANUP={}", serde_json::json!({
                "attempt": attempt,
                "child_ready": was_active,
                "directory_left_behind": left_behind,
            }));
            // clean only this regression's known fixture paths after a failing pre-fix attempt.
            tokio::time::timeout(Duration::from_secs(2), async {
                while path.exists() {
                    let _ = std::fs::remove_file(path.join("backend/.env"));
                    let _ = std::fs::remove_dir(path.join("backend"));
                    let _ = std::fs::remove_dir(&path);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(was_active, "the deadline must fire only after the child confirms it is active");
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        }
        assert_eq!(leaked, 0, "active-child deadline left working directories behind");
    }
}
