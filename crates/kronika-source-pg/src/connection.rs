//! Shared credential-safe endpoint identity and monitored `PostgreSQL` connection setup.

use crate::{CONNECT_TIMEOUT, Transport, query};
use std::net::IpAddr;
use tokio_postgres::{Client, Config, config::Host};

const DEFAULT_PORT: u16 = 5432;

pub(crate) struct MonitoringConnection {
    pub(crate) client: Client,
    pub(crate) driver: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Copy)]
pub(crate) enum ConnectStage {
    Connect,
    Configure,
}

pub(crate) enum ConnectionFailure {
    PostgreSql(tokio_postgres::Error),
    Timeout(tokio::time::error::Elapsed),
}

pub(crate) struct FailedConnection {
    pub(crate) stage: ConnectStage,
    pub(crate) error: ConnectionFailure,
}

pub(crate) async fn connect_monitoring(
    config: &Config,
    transport: &Transport,
) -> Result<MonitoringConnection, FailedConnection> {
    let (client, connection) = tokio::time::timeout(CONNECT_TIMEOUT, transport.connect(config))
        .await
        .map_err(|error| FailedConnection {
            stage: ConnectStage::Connect,
            error: ConnectionFailure::Timeout(error),
        })?
        .map_err(|error| FailedConnection {
            stage: ConnectStage::Connect,
            error: ConnectionFailure::PostgreSql(error),
        })?;
    let driver = tokio::spawn(async move {
        let _ended = connection.await;
    });
    let configured = tokio::time::timeout(CONNECT_TIMEOUT, query::configure_session(&client)).await;
    let error = match configured {
        Ok(Ok(())) => return Ok(MonitoringConnection { client, driver }),
        Ok(Err(error)) => ConnectionFailure::PostgreSql(error),
        Err(error) => ConnectionFailure::Timeout(error),
    };
    driver.abort();
    Err(FailedConnection {
        stage: ConnectStage::Configure,
        error,
    })
}

pub(crate) fn connection_label(config: &Config, user: Option<&str>, source_index: usize) -> String {
    let user = user.unwrap_or("server-default");
    let ports = config.get_ports();
    let endpoints = if config.get_hosts().is_empty() {
        config
            .get_hostaddrs()
            .iter()
            .enumerate()
            .map(|(index, host)| endpoint(user, &ip_label(*host), port_at(ports, index)))
            .collect::<Vec<_>>()
    } else {
        config
            .get_hosts()
            .iter()
            .enumerate()
            .map(|(index, host)| {
                let host = match host {
                    Host::Tcp(host) => tcp_label(host),
                    #[cfg(unix)]
                    Host::Unix(path) => format!("unix:{}", path.display()),
                };
                endpoint(user, &host, port_at(ports, index))
            })
            .collect::<Vec<_>>()
    };
    if endpoints.is_empty() {
        format!("{user}@source[{source_index}]")
    } else {
        endpoints.join(",")
    }
}

fn port_at(ports: &[u16], index: usize) -> u16 {
    match ports {
        [] => DEFAULT_PORT,
        [port] => *port,
        many => many.get(index).copied().unwrap_or(DEFAULT_PORT),
    }
}

fn endpoint(user: &str, host: &str, port: u16) -> String {
    format!("{user}@{host}:{port}")
}

fn tcp_label(host: &str) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        host.to_owned()
    } else if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn ip_label(host: IpAddr) -> String {
    match host {
        IpAddr::V4(host) => host.to_string(),
        IpAddr::V6(host) => format!("[{host}]"),
    }
}
