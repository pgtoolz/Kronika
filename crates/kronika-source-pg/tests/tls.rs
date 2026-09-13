//! Disposable `PostgreSQL` TLS acceptance; never connects unless explicitly selected.

#![allow(
    unused_crate_dependencies,
    reason = "this integration test reaches implementation dependencies through the production library"
)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use kronika_source_pg::query::{self, QueryStats};
use kronika_source_pg::{Pool, Transport};
use tokio_postgres::Config;
use tokio_postgres::config::SslMode;

#[tokio::test]
#[ignore = "requires the explicitly provisioned disposable TLS PostgreSQL fixture"]
async fn verified_tls_pool_reconnect_cancel_and_plaintext_contract() {
    let dsn = std::env::var("KRONIKA_TEST_TLS_DSN").expect("set disposable TLS DSN");
    let config: Config = dsn.parse().expect("parse fixture DSN");
    assert_eq!(config.get_ssl_mode(), SslMode::Require);
    let transport = Transport::from_env().expect("load test CA using KRONIKA_PG_SSL_ROOT_CERT");
    let (observer, driver) = transport
        .connect(&config)
        .await
        .expect("verified observer TLS");
    let driver = tokio::spawn(driver);
    query::configure_session(&observer)
        .await
        .expect("observer session limits");
    let mut pool = Pool::new(&dsn).expect("pool uses configured CA");
    for expected_generation in [1, 2] {
        let session = pool
            .session()
            .await
            .expect("verified pooled session/reconnect");
        assert_eq!(session.generation(), expected_generation);
        let mut stats = QueryStats::default();
        let verified = query::read_simple_i32(session,
            "SELECT (ssl AND current_setting('lock_timeout') = '100ms' AND current_setting('statement_timeout') = '30s')::int FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &mut stats).await.expect("TLS and both session limits");
        assert_eq!(verified, 1);
        if expected_generation == 1 {
            pool.close();
        }
    }
    let session = pool.session().await.expect("reuse TLS session for cancel");
    let mut stats = QueryStats::default();
    let backend = query::read_simple_i32(session, "SELECT pg_backend_pid()", &mut stats)
        .await
        .expect("query backend identity");
    let cancelled = query::timeout(
        session,
        Duration::from_millis(150),
        query::read_simple_i32(session, "SELECT 1 FROM pg_sleep(10)", &mut stats),
    )
    .await;
    assert!(cancelled.is_err(), "client backstop elapsed");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let idle: bool = observer.query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND state = 'idle')", &[&backend]
            ).await.expect("observe cancellation result").get(0);
            if idle { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("TLS CancelRequest interrupts pg_sleep before ten seconds");
    pool.close();

    assert!(
        Transport::default().connect(&config).await.is_err(),
        "public roots do not trust the fixture's private CA"
    );
    let mut wrong_name = Config::new();
    wrong_name
        .host("not-the-fixture.invalid")
        .hostaddr(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .port(config.get_ports()[0])
        .user(config.get_user().expect("fixture user"))
        .dbname(config.get_dbname().expect("fixture database"))
        .ssl_mode(SslMode::Require);
    if let Some(password) = config.get_password() {
        wrong_name.password(password);
    }
    assert!(
        transport.connect(&wrong_name).await.is_err(),
        "trusted CA does not bypass hostname verification"
    );

    let mut plain = config;
    plain.ssl_mode(SslMode::Disable);
    let (client, plaintext_driver) = transport
        .connect(&plain)
        .await
        .expect("explicit plaintext retained");
    let plaintext_driver = tokio::spawn(plaintext_driver);
    let ssl: bool = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await
        .expect("inspect explicit plaintext session")
        .get(0);
    assert!(!ssl);
    drop(client);
    plaintext_driver.abort();
    drop(observer);
    driver.abort();
}
