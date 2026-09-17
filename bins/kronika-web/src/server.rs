//! Accept HTTP connections and dispatch admitted requests to their handlers.

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use hyper::header::{CACHE_CONTROL, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use tokio::net::TcpListener;

use crate::api::ApiError;
use crate::body::WebBody;
use crate::config::Config;
use crate::request::{RequestTarget, if_none_match_values, route_request, session_response};
use crate::response::{failed, json_response};
use crate::{export, mcp, query_adapter, route, streaming, ui};

#[tokio::main(worker_threads = 2)]
pub(crate) async fn run() -> Result<()> {
    let config = Arc::new(Config::from_env()?);
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("listen on {}", config.listen))?;
    println!("ready {}", config.listen);

    loop {
        let (stream, _peer) = listener.accept().await.context("accept a connection")?;
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let service = service_fn(move |request| answer(Arc::clone(&config), request));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("kronika-web: connection ended: {error}");
            }
        });
    }
}

async fn answer(
    config: Arc<Config>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<WebBody>, Infallible> {
    let target = match route_request(config.account.as_ref(), &request) {
        Ok(target) => target,
        Err(error) => return Ok(error.response()),
    };
    let if_none_match = if_none_match_values(request.headers());
    Ok(match target {
        RequestTarget::Ui { head, coding } => ui::response(head, if_none_match.as_deref(), coding)
            .unwrap_or_else(|error| {
                eprintln!("kronika-web: serve embedded interface: {error}");
                failed()
            }),
        RequestTarget::Session(session) => {
            session_response(config.account.as_ref(), session).unwrap_or_else(failed)
        }
        RequestTarget::Api { route, accepted } => match route {
            route::Route::Export(range) => {
                export::response(
                    config.data_root.clone(),
                    range,
                    Arc::clone(&config.export_gate),
                )
                .await
            }
            route::Route::McpAccess => mcp_access(&config),
            route::Route::InstanceLabel => instance_label(config).await,
            route @ route::Route::Recorded(_) => {
                streaming::response(config, route, if_none_match, accepted).await
            }
        },
        RequestTarget::Mcp => mcp::response(config, request).await,
    })
}

/// Return the configured MCP credential only after request admission succeeds.
pub(crate) fn mcp_access(config: &Config) -> Response<WebBody> {
    use base64::Engine as _;
    let authorization = config.account.as_ref().map(|account| {
        let credentials = format!("{}:{}", account.user, account.password);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        )
    });
    mcp::with_private_headers(json_response(
        StatusCode::OK,
        json!({ "record": "mcp_access", "authorization": authorization }).to_string(),
    ))
}

/// Cache a recorded database label for a day; retry missing or unreadable data next time.
pub(crate) async fn instance_label(config: Arc<Config>) -> Response<WebBody> {
    let database = match tokio::task::spawn_blocking(move || largest_database(&config)).await {
        Ok(Ok(database)) => database,
        Ok(Err(error)) => {
            eprintln!("kronika-web: instance label: {error}");
            None
        }
        Err(error) => {
            eprintln!("kronika-web: instance label task: {error}");
            return failed();
        }
    };
    let mut response = json_response(
        StatusCode::OK,
        json!({ "record": "instance_label", "database": database }).to_string(),
    );
    if database.is_some() {
        response.headers_mut().insert(
            CACHE_CONTROL,
            HeaderValue::from_static("private,max-age=86400"),
        );
    }
    response
}

/// Choose the largest database in the newest recorded relation snapshot.
fn largest_database(config: &Config) -> Result<Option<String>, ApiError> {
    let dataset = Arc::new(query_adapter::NativeDataset::from_root(&config.data_root)?);
    let context = kronika_query::QueryContext::new(dataset, config.sources, config.synthetic_demo);
    let query = kronika_query::snapshot::CurrentSnapshotQuery {
        logical_name: "pg_stat_user_tables".to_owned(),
        fields: vec!["displayed_storage_bytes".to_owned()],
        order: Some(kronika_query::snapshot::FinderOrder {
            field: "displayed_storage_bytes".to_owned(),
            direction: kronika_query::Order::Desc,
        }),
        group: Some(route::RelationGroup::Database),
        limit: 1,
    };
    let Some(result) =
        kronika_query::snapshot::execute_current_relation(&context, query, &|| false)?
    else {
        return Ok(None);
    };
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.key.text("datname").map(str::to_owned)))
}
