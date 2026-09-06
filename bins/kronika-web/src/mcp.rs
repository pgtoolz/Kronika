//! Stateless, tools-only MCP transport over Streamable HTTP.
//!
//! Results are structured-content only: `content` carries a one-line
//! summary and every row lives in `structuredContent` — a client that
//! ignores structured content gets no rows. Cancellation covers HTTP
//! response abandonment only: the stateless transport keeps no request-id
//! state, so a client-issued `notifications/cancelled` cannot reach an
//! in-flight call.

use std::sync::Arc;

use http_body_util::BodyExt as _;
use hyper::body::Body;
use hyper::header::{CACHE_CONTROL, HeaderValue, VARY};
use hyper::{Request, Response};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use kronika_query::{QueryContext, QueryError};

use crate::WebBody;
use crate::body::BodyError;
use crate::config::Config;
use crate::query_adapter::NativeDataset;

mod catalog;
mod context;
mod dispatch;
mod events;
mod filter;
mod instance;
mod overview;
mod postgresql;
mod processes;
mod row_detail;
mod semantics;
mod time;

#[cfg(test)]
mod finder_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
fn test_config(data_root: std::path::PathBuf) -> Arc<Config> {
    Arc::new(Config {
        data_root,
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: crate::config::Account {
            user: "dba".to_owned(),
            password: "secret".to_owned(),
        },
        authentication_required: true,
        sources: crate::config::SOURCE_OS | crate::config::SOURCE_POSTGRESQL,
        synthetic_demo: false,
        export_gate: Arc::new(tokio::sync::Semaphore::new(1)),
    })
}

/// Runs one typed snapshot query against a fresh native capture, replaying once
/// when the active source rolls over during the read.
fn run_snapshot_query<R>(
    config: &Config,
    mut execute: impl FnMut(&QueryContext) -> Result<R, QueryError>,
) -> Result<R, crate::api::ApiError> {
    let mut attempt = || {
        let dataset = Arc::new(NativeDataset::from_root(&config.data_root)?);
        let context = QueryContext::new(dataset, config.sources, config.synthetic_demo);
        execute(&context)
    };
    match attempt() {
        Err(error) if error.source_changed_during_read() => attempt(),
        result => result,
    }
}

fn run_finder_query<R>(
    config: &Config,
    logical_name: &str,
    execute: impl FnMut(&QueryContext) -> Result<R, QueryError>,
    render: impl FnOnce(R) -> CallToolResult,
) -> CallToolResult {
    match run_snapshot_query(config, execute) {
        Ok(result) => render(result),
        Err(error) => semantics::finder_storage_error(logical_name, &error),
    }
}

#[derive(Clone)]
struct KronikaMcp {
    config: Arc<Config>,
}

// Unadvertised prompt and resource methods use rmcp's default responses.
impl ServerHandler for KronikaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("kronika", env!("CARGO_PKG_VERSION")).with_title("Kronika"),
            )
            .with_instructions(catalog::SERVER_INSTRUCTIONS)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(catalog::tools())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private)))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send + '_ {
        let config = Arc::clone(&self.config);
        async move {
            // Segment reads and decoding block; keep them off Tokio worker
            // threads. The request's cancellation token rides along so an
            // abandoned request stops scanning.
            let token = context.ct.clone();
            let result = match tokio::task::spawn_blocking(move || {
                dispatch::dispatch(&config, request, &|| token.is_cancelled())
            })
            .await
            {
                Ok(result) => result?,
                // The panic detail stays in the server log; the caller gets
                // a stable internal error without it.
                Err(join_error) => {
                    eprintln!("kronika-web: mcp dispatch task failed: {join_error}");
                    return Err(rmcp::ErrorData::internal_error(
                        "tool dispatch failed",
                        None,
                    ));
                }
            };
            Ok(CallToolResponse::from(result))
        }
    }
}

/// Handles an already-authenticated request with stateless Streamable HTTP.
pub(crate) async fn response<B>(config: Arc<Config>, request: Request<B>) -> Response<WebBody>
where
    B: Body + Send + 'static,
    B::Error: std::fmt::Display,
{
    let service = StreamableHttpService::new(
        move || {
            Ok(KronikaMcp {
                config: Arc::clone(&config),
            })
        },
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts()
            .disable_allowed_origins(),
    );
    let response = service.handle(request).await;
    let (parts, body) = response.into_parts();
    let response = Response::from_parts(parts, body.map_err(BodyError::from).boxed_unsync());
    with_private_headers(response)
}

/// Makes MCP responses private, non-cacheable, and varied by authentication headers.
pub(crate) fn with_private_headers(mut response: Response<WebBody>) -> Response<WebBody> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private,no-store"));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
    response
}
