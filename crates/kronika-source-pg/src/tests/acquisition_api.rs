use std::net::TcpListener;

use crate::query::BatchWrite;
use crate::{PgCollectionSelection, PgCollector, Pool, Transport};

#[tokio::test]
async fn empty_selection_never_opens_a_connection_or_admits_a_batch() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("source endpoint");
    listener
        .set_nonblocking(true)
        .expect("nonblocking endpoint");
    let port = listener.local_addr().expect("endpoint address").port();
    let pool = Pool::with_transport(
        &format!("host=127.0.0.1 port={port} user=monitor sslmode=disable"),
        Transport::default(),
    )
    .expect("explicit connection policy");
    let mut collector = PgCollector::new(pool);
    collector
        .collect(
            &PgCollectionSelection::default(),
            &mut |_| panic!("an empty selection must not produce observations"),
            |_, _| -> Result<BatchWrite, ()> {
                panic!("an empty selection must not produce batches")
            },
        )
        .await
        .expect("empty acquisition");
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn discovery_is_not_intact_while_the_primary_is_down() {
    let pool = Pool::with_transport(
        "host=127.0.0.1 port=1 user=monitor sslmode=disable",
        Transport::default(),
    )
    .expect("explicit connection policy");
    let mut collector = PgCollector::new(pool);
    // never opened: no generation, so an empty list is not authoritative
    assert!(!collector.discovery_intact());
    assert!(collector.discovered_database_names().is_empty());
    collector.close_connections();
    assert!(!collector.discovery_intact());
}
