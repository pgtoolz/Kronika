use std::sync::Arc;

use crate::config::Config;

mod finders;
mod transport;

pub(super) fn test_config(data_root: std::path::PathBuf) -> Arc<Config> {
    Arc::new(Config {
        data_root,
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: Some(crate::config::Account {
            user: "dba".to_owned(),
            password: "secret".to_owned(),
        }),
        sources: crate::config::SOURCE_OS | crate::config::SOURCE_POSTGRESQL,
        synthetic_demo: false,
        export_gate: Arc::new(tokio::sync::Semaphore::new(1)),
    })
}
