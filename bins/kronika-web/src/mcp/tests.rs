use std::sync::Arc;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use serde_json::json;

use super::{response, test_config};
use crate::config::Config;
use crate::tests::artifacts::Fixture;

const FORBIDDEN_COORDINATE_KEYS: [&str; 5] = [
    "detail_locator",
    "type_id",
    "segment_id",
    "row_ordinal",
    "row_key",
];

fn assert_no_storage_coordinate_keys(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for forbidden in FORBIDDEN_COORDINATE_KEYS {
                assert!(
                    !object.contains_key(forbidden),
                    "public MCP result exposed {forbidden}: {value:#?}",
                );
            }
            for child in object.values() {
                assert_no_storage_coordinate_keys(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                assert_no_storage_coordinate_keys(child);
            }
        }
        _ => {}
    }
}

fn assert_no_detail_ref_property(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            assert!(
                !object.contains_key("detail_ref"),
                "error emitted detail_ref"
            );
            for child in object.values() {
                assert_no_detail_ref_property(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                assert_no_detail_ref_property(child);
            }
        }
        _ => {}
    }
}

fn assert_json_content_matches_structured(result: &rmcp::model::CallToolResult) {
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.content.len(), 1);
    let text = &result.content[0].as_text().expect("JSON text").text;
    let content: serde_json::Value = serde_json::from_str(text).expect("content JSON");
    assert_eq!(Some(&content), result.structured_content.as_ref());
}

fn assert_wire_json_content_matches_structured(response: &serde_json::Value) {
    let content = response["result"]["content"]
        .as_array()
        .expect("wire content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    let text = content[0]["text"].as_str().expect("wire JSON text");
    let parsed: serde_json::Value = serde_json::from_str(text).expect("wire content JSON");
    assert_eq!(parsed, response["result"]["structuredContent"]);
}

fn assert_detail_ref(row: &serde_json::Value, section: &str, detail_only_fields: &[&str]) {
    let row = row.as_object().expect("finder row object");
    for field in detail_only_fields {
        assert!(
            !row.contains_key(*field),
            "mass row exposed {section}.{field}"
        );
    }
    assert!(
        row["detail_ref"]
            .as_str()
            .is_some_and(|detail_ref| !detail_ref.is_empty()),
        "{section} row has one opaque detail_ref",
    );
    assert_no_storage_coordinate_keys(&serde_json::Value::Object(row.clone()));
}

fn detail_arguments(row: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    json!({
        "detail_ref": row["detail_ref"].as_str().expect("detail_ref"),
    })
    .as_object()
    .expect("detail arguments")
    .clone()
}

fn assert_detail_roundtrip(
    config: &Config,
    row: &serde_json::Value,
    field: &str,
    expected: &serde_json::Value,
) {
    let detail = crate::mcp::row_detail::call(config, detail_arguments(row), &|| false);
    assert_eq!(detail.is_error, Some(false));
    let detail = detail.structured_content.expect("detail row");
    assert_eq!(&detail[field], expected);
    assert_no_storage_coordinate_keys(&detail);
}

fn streamed(prepared: crate::api::Prepared) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    prepared
        .stream(
            &mut |record| {
                records.push(serde_json::from_slice(&record).expect("JSON record"));
                true
            },
            &|| false,
        )
        .expect("stream resource");
    records
}

fn assert_opaque_tool_contract(tools: &[serde_json::Value]) {
    for tool in tools {
        assert_no_storage_coordinate_keys(&tool["inputSchema"]);
        if let Some(output) = tool.get("outputSchema") {
            assert_no_storage_coordinate_keys(output);
        }
        let description = tool["description"].as_str().expect("description");
        for forbidden in FORBIDDEN_COORDINATE_KEYS {
            assert!(
                !description.contains(forbidden),
                "{} description exposed {forbidden}",
                tool["name"],
            );
        }
    }
    let detail = tools
        .iter()
        .find(|tool| tool["name"] == "kronika_get_row_detail")
        .expect("row detail tool");
    assert_eq!(detail["inputSchema"]["required"], json!(["detail_ref"]));
    assert_eq!(
        detail["inputSchema"]["properties"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1),
    );
    assert_eq!(
        detail["inputSchema"]["properties"]["detail_ref"]["type"],
        "string",
    );
}

async fn rpc_response(
    config: Arc<Config>,
    body: serde_json::Value,
    protocol_version: Option<&str>,
    mcp_method: Option<&str>,
) -> serde_json::Value {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream");
    if let Some(protocol_version) = protocol_version {
        builder = builder.header("MCP-Protocol-Version", protocol_version);
    }
    if let Some(mcp_method) = mcp_method {
        builder = builder.header("Mcp-Method", mcp_method);
    }
    let request = builder
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");
    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json-rpc response")
}

#[tokio::test]
async fn initialize_and_discover_publish_the_same_server_instructions() {
    let expected = crate::mcp::catalog::SERVER_INSTRUCTIONS;
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let response = rpc_response(
            test_config(std::env::temp_dir()),
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": version,
                    "capabilities": {},
                    "clientInfo": {"name": "test-client", "version": "1.0"}
                }
            }),
            None,
            None,
        )
        .await;
        assert_eq!(response["result"]["instructions"], expected, "{version}");
        assert_eq!(response["result"]["serverInfo"]["name"], "kronika");
        assert_eq!(response["result"]["serverInfo"]["title"], "Kronika");
        assert_eq!(
            response["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    let response = rpc_response(
        test_config(std::env::temp_dir()),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name": "test-client", "version": "1.0"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }}
        }),
        Some("2026-07-28"),
        Some("server/discover"),
    )
    .await;
    assert_eq!(response["result"]["instructions"], expected);
    assert_eq!(
        response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "kronika"
    );
    assert_eq!(
        response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::test]
async fn tools_list_returns_the_fourteen_tool_catalog() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(test_config(std::env::temp_dir()), request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    assert_eq!(decoded["result"]["ttlMs"], json!(0));
    assert_eq!(decoded["result"]["cacheScope"], json!("private"));
    let tools = decoded["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "kronika_rank_metrics",
            "kronika_list_recorded_sections",
            "kronika_get_instance",
            "kronika_find_postgresql_tables",
            "kronika_find_postgresql_indexes",
            "kronika_find_postgresql_activity",
            "kronika_find_postgresql_locks",
            "kronika_find_postgresql_vacuum",
            "kronika_find_postgresql_databases",
            "kronika_find_postgresql_statements",
            "kronika_find_postgresql_plans",
            "kronika_find_processes",
            "kronika_get_row_detail",
            "kronika_find_events",
        ]
    );
    assert_public_tool_contract(tools);
}

fn assert_public_tool_contract(tools: &[serde_json::Value]) {
    for old_name in ["kronika_overview", "kronika_get_context"] {
        assert!(
            !tools.iter().any(|tool| tool["name"] == old_name),
            "obsolete tool remains published: {old_name}"
        );
    }
    assert_tool_descriptions(tools);
    for tool in tools {
        let output_schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("{} is missing outputSchema", tool["name"]));
        assert_eq!(
            output_schema["type"],
            json!("object"),
            "{} outputSchema.type",
            tool["name"]
        );
    }
    assert_opaque_tool_contract(tools);
    let overview = tools
        .iter()
        .find(|tool| tool["name"] == "kronika_rank_metrics")
        .expect("Overview tool");
    assert!(overview["outputSchema"].is_object());
    let overview_description = overview["description"].as_str().expect("description");
    assert!(!overview_description.contains("working"));
    assert!(overview_description.contains("separate result"));
    assert!(!overview_description.contains("summed per row"));
    assert!(!overview_description.contains("scan state"));
    let events = tools
        .iter()
        .find(|tool| tool["name"] == "kronika_find_events")
        .expect("Events tool");
    assert!(events["outputSchema"].is_object());
    let events_schema = &events["inputSchema"];
    assert_eq!(events_schema["additionalProperties"], false);
    assert_eq!(
        events_schema["properties"]["representation"]["default"],
        "groups"
    );
    assert_eq!(events_schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(events_schema["properties"]["limit"]["maximum"], 5_000);
    assert_eq!(
        events_schema["properties"]["sources"]["type"],
        json!(["array", "null"])
    );
}

fn assert_tool_descriptions(tools: &[serde_json::Value]) {
    for (name, required_text) in [
        (
            "kronika_rank_metrics",
            "Each requested field produces a separate result",
        ),
        (
            "kronika_list_recorded_sections",
            "Lists the recorded time bounds",
        ),
        ("kronika_get_instance", "latest recorded host metadata"),
        (
            "kronika_find_postgresql_tables",
            "Filters are applied before aggregation",
        ),
        (
            "kronika_find_postgresql_indexes",
            "Filters are applied before aggregation",
        ),
        ("kronika_find_postgresql_activity", "including state, waits"),
        ("kronika_find_postgresql_locks", "direct blocker PIDs"),
        ("kronika_find_postgresql_vacuum", "dead-tuple storage"),
        ("kronika_find_postgresql_databases", "Missing predecessors"),
        (
            "kronika_find_postgresql_statements",
            "Derived fields expose per-call values",
        ),
        (
            "kronika_find_postgresql_plans",
            "`calls_per_second` is its interval rate",
        ),
        (
            "kronika_find_processes",
            "compatible observations of the same process",
        ),
        (
            "kronika_get_row_detail",
            "{stored_text, full_len, truncated, sha256}",
        ),
        ("kronika_find_events", "`groups` returns merged summaries"),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .expect("named tool");
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|description| description.contains(required_text)),
            "{name} description"
        );
    }
    for forbidden in [
        "entry point",
        "read it once per session",
        "web Events console",
        "request-budget refusal",
        "pre-ranking scan",
        "text ceiling",
        "max_plan_length",
        "WAL finalization",
    ] {
        for tool in tools {
            assert!(
                !tool["description"]
                    .as_str()
                    .is_some_and(|description| description.contains(forbidden)),
                "{} description contains {forbidden}",
                tool["name"]
            );
        }
    }
}

#[test]
fn get_context_lists_recorded_sections_with_row_counts() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let sections = structured["sections"].as_array().expect("sections array");
    let os_process = sections
        .iter()
        .find(|section| section["logical_name"] == "os_process")
        .expect("os_process section present");
    assert_eq!(os_process["rows"], "2");
    assert_eq!(os_process["source_family"], "os");
}

#[test]
fn get_context_reports_no_sections_on_an_empty_data_root() {
    let empty_root = tempfile::tempdir().expect("empty data root");
    let config = test_config(empty_root.path().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["sections"].as_array(), Some(&Vec::new()));
}

// Two samples per PID make ranking maxima differ from last-value band totals.
fn ranked_process_gauge_rows() -> [(i64, i32, i64, &'static str); 10] {
    [
        (100, 101, 50, "fixture"),
        (300, 101, 10, "fixture"),
        (100, 102, 40, "fixture"),
        (300, 102, 45, "fixture"),
        (100, 103, 5, "fixture"),
        (300, 103, 30, "fixture"),
        (100, 104, 20, "fixture"),
        (300, 104, 8, "fixture"),
        (100, 105, 15, "fixture"),
        (300, 105, 25, "fixture"),
    ]
}

fn overview_arguments(
    section: &str,
    fields: &[&str],
    from: i64,
    to: i64,
    top: usize,
) -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "from": from,
        "to": to,
        "rankings": [{"section": section, "fields": fields, "top": top}],
    })
    .as_object()
    .expect("object")
    .clone()
}

fn mount_entity<'a>(overview: &'a serde_json::Value, mount: &str) -> &'a serde_json::Value {
    overview["results"][0]["entities"]
        .as_array()
        .expect("Overview entities")
        .iter()
        .find(|entity| {
            entity["identity"]["mount_point"]["stored_text"] == mount
                || entity["identity"]["mount_point"] == mount
        })
        .unwrap_or_else(|| panic!("{mount} entity in {overview:#?}"))
}

#[test]
fn overview_ranks_the_top_entities_and_reports_the_others_total() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("os_process", &["rmem_kb"], 100, 400, 2);

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let ranking = &structured["results"][0];
    assert_eq!(ranking["summary"], "max");
    let entities = ranking["entities"].as_array().expect("entities array");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["total"], 50.0);
    assert_eq!(entities[0]["identity"]["pid"], 101);
    assert_eq!(entities[1]["total"], 45.0);
    // One convention for every number in a gauge ranking: entity totals
    // are window maxima, so the two aggregate fields are maxima too —
    // totals_total across all five entities, others_total across the
    // three beyond top=2.
    assert_eq!(ranking["totals_total"], 50.0);
    assert_eq!(ranking["others_total"], 30.0);
    assert_eq!(ranking["entity_count"], "5");
    assert!(ranking.get("grid").is_none());
    assert!(entities.iter().all(|entity| entity.get("cells").is_none()));
}

#[test]
fn mount_detail_refs_survive_active_segment_finalization_and_reordering() {
    let mut fixture = Fixture::new();
    for at in [100, 200, 300] {
        fixture.append_reordering_mount_snapshot(at);
    }
    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("os_mountinfo", &["total_bytes"], 100, 301, 2);

    let active = crate::mcp::overview::call(&config, arguments.clone(), &|| false);
    assert_eq!(active.is_error, Some(false));
    let active = active.structured_content.expect("active Overview");
    let pgdata = mount_entity(&active, "/pgdata");
    let victim = mount_entity(&active, "/victim");
    assert_detail_ref(pgdata, "os_mountinfo", &[]);
    assert_detail_ref(victim, "os_mountinfo", &[]);

    let active_detail = crate::mcp::row_detail::call(&config, detail_arguments(pgdata), &|| false);
    assert_eq!(active_detail.is_error, Some(false));
    let active_detail = active_detail.structured_content.expect("active detail");
    assert_eq!(active_detail["mount_point"]["stored_text"], "/pgdata");
    assert_no_storage_coordinate_keys(&active_detail);

    let pgdata_ref = pgdata["detail_ref"]
        .as_str()
        .expect("pgdata ref")
        .to_owned();
    let victim_ref = victim["detail_ref"]
        .as_str()
        .expect("victim ref")
        .to_owned();
    fixture.finish_and_continue(1_709_164_800_001_000);

    let finished_pgdata = crate::mcp::row_detail::call(
        &config,
        json!({"detail_ref": pgdata_ref})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(finished_pgdata.is_error, Some(false));
    let finished_pgdata = finished_pgdata
        .structured_content
        .expect("finished pgdata detail");
    assert_eq!(finished_pgdata["mount_point"]["stored_text"], "/pgdata");
    assert_eq!(finished_pgdata["major"], active_detail["major"]);
    assert_eq!(finished_pgdata["minor"], active_detail["minor"]);
    assert_no_storage_coordinate_keys(&finished_pgdata);

    // The old positional hint now points at /pgdata, but the copied reference
    // must still resolve the same logical /victim row.
    let finished_victim = crate::mcp::row_detail::call(
        &config,
        json!({"detail_ref": victim_ref})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(finished_victim.is_error, Some(false));
    let finished_victim = finished_victim
        .structured_content
        .expect("finished victim detail");
    assert_eq!(finished_victim["mount_point"], "/victim");
    assert_no_storage_coordinate_keys(&finished_victim);

    let finished = crate::mcp::overview::call(&config, arguments, &|| false);
    assert_eq!(finished.is_error, Some(false));
    let finished = finished.structured_content.expect("finished Overview");
    let fresh_pgdata = mount_entity(&finished, "/pgdata");
    assert_detail_ref(fresh_pgdata, "os_mountinfo", &[]);
    let direct = crate::mcp::row_detail::call(&config, detail_arguments(fresh_pgdata), &|| false);
    assert_eq!(direct.is_error, Some(false));
    assert_eq!(
        direct.structured_content.expect("fresh detail")["mount_point"]["stored_text"],
        "/pgdata"
    );

    let rejected = crate::mcp::row_detail::call(
        &config,
        json!({"detail_ref": format!("{pgdata_ref}A")})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(rejected.is_error, Some(true));
    let message = &rejected.content[0].as_text().expect("error").text;
    assert!(message.contains("invalid detail_ref"), "{message}");
    for forbidden in FORBIDDEN_COORDINATE_KEYS {
        assert!(!message.contains(forbidden), "error exposed {forbidden}");
    }
}

#[test]
fn overview_resolves_an_interleaved_equivalent_event_identity() {
    let mut fixture = Fixture::new();
    fixture.append_log_error_count(100, 1);
    fixture.append_log_error_count(100, 2);
    fixture.append_log_error_count(100, 1);
    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("pg_log_errors", &["count"], 0, 101, 1);

    let overview = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(overview.is_error, Some(false));
    let overview = overview.structured_content.expect("Overview");
    let entity = &overview["results"][0]["entities"][0];
    assert_detail_ref(entity, "pg_log_errors", &[]);
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(entity), &|| false);
    assert_eq!(detail.is_error, Some(false));
    assert_eq!(detail.structured_content.expect("detail")["count"], 1);
}

#[test]
fn overview_emits_a_unique_final_event_after_an_older_duplicate() {
    let mut fixture = Fixture::new();
    fixture.append_log_error_count(100, 1);
    fixture.append_log_error_count(100, 1);
    fixture.append_log_error_count(100, 2);
    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("pg_log_errors", &["count"], 0, 101, 1);

    let overview = crate::mcp::overview::call(&config, arguments, &|| false);
    assert_eq!(overview.is_error, Some(false));
    let overview = overview.structured_content.expect("Overview");
    let entity = &overview["results"][0]["entities"][0];
    assert_detail_ref(entity, "pg_log_errors", &[]);
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(entity), &|| false);

    assert_eq!(detail.is_error, Some(false));
    assert_eq!(detail.structured_content.expect("detail")["count"], 2);
}

#[test]
fn overview_returns_the_five_statement_rankings_in_one_ordered_batch() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());
    let fields = [
        "total_exec_time",
        "calls",
        "shared_blks_read",
        "temp_blks_written",
        "wal_bytes",
    ];
    let rankings = fields
        .iter()
        .map(|field| json!({"section": "pg_stat_statements", "fields": [field], "top": 10}))
        .collect::<Vec<_>>();
    let arguments = json!({"from": 100, "to": 201, "rankings": rankings})
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let results = structured["results"].as_array().expect("ranking results");
    assert_eq!(results.len(), fields.len());
    for (result, field) in results.iter().zip(fields) {
        assert_eq!(result["ranking"]["fields"], json!([field]));
        assert_eq!(result["entity_count"], "3");
        assert_eq!(result["entities"].as_array().expect("entities").len(), 3);
        assert!(
            result["entities"]
                .as_array()
                .expect("entities")
                .iter()
                .all(|entity| entity["labels"].get("query").is_none()
                    && entity["labels"].get("datname").is_some()
                    && entity["labels"].get("usename").is_some()
                    && entity["detail_ref"].is_string())
        );
    }
    assert_no_storage_coordinate_keys(&structured);
    let entity = &results[0]["entities"][0];
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(entity), &|| false);
    assert_eq!(detail.is_error, Some(false));
    let detail = detail.structured_content.expect("statement detail");
    assert!(detail["query"]["stored_text"].is_string());
    assert_eq!(
        detail["query"].as_object().map(serde_json::Map::len),
        Some(4)
    );
}

#[test]
fn overview_batches_os_gauge_os_counter_and_postgresql_in_order() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 10, "db", "public", "alpha"),
        (300, 1, 11, 20, "db", "public", "alpha"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = json!({
        "from": 100,
        "to": 400,
        "rankings": [
            {"section": "os_process", "fields": ["rmem_kb"], "top": 1},
            {"section": "os_process", "fields": ["utime"], "top": 1},
            {"section": "pg_stat_user_tables", "fields": ["seq_scan"], "top": 1},
        ],
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rankings = structured["results"].as_array().expect("rankings");
    assert_eq!(rankings.len(), 3);
    assert_eq!(rankings[0]["ranking"]["section"], "os_process");
    assert_eq!(rankings[0]["class"], "gauge");
    assert_eq!(rankings[1]["ranking"]["fields"], json!(["utime"]));
    assert_eq!(rankings[1]["class"], "cumulative");
    assert_eq!(rankings[2]["ranking"]["section"], "pg_stat_user_tables");
    assert_eq!(rankings[2]["entities"][0]["total"], 10.0);
}

#[test]
fn overview_empty_window_reports_only_no_data_and_its_row_count() {
    let mut fixture = Fixture::new();
    // Rows at 50 and 400 widen the recorded range past the neighbours at
    // 100 and 300.
    let mut rows = vec![(50, 101, 1, "fixture")];
    rows.extend_from_slice(&ranked_process_gauge_rows());
    rows.push((400, 101, 1, "fixture"));
    fixture.append_process_gauge_rows(&rows);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("os_process", &["rmem_kb"], 150, 200, 5);

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let ranking = &structured["results"][0];
    assert_eq!(ranking["entity_count"], "0");
    assert_eq!(ranking["entities"].as_array().expect("entities").len(), 0);
    assert_eq!(ranking["coverage"]["state"], "no_data");
    assert_eq!(ranking["coverage"]["window_rows"], "0");
    for removed in [
        "as_of",
        "recorded_from",
        "recorded_to",
        "nearest_row_before",
        "nearest_row_after",
    ] {
        assert!(
            ranking.get(removed).is_none() && ranking["coverage"].get(removed).is_none(),
            "removed Overview time field {removed}"
        );
    }
}

#[test]
fn overview_reports_in_window_rows_whose_fields_rank_nothing() {
    let mut fixture = Fixture::new();
    // The fixture leaves toast_bytes null, the recorded state of a table
    // that never had a TOAST relation.
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 10, "db", "public", "alpha"),
        (200, 1, 11, 20, "db", "public", "alpha"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("pg_stat_user_tables", &["toast_bytes"], 100, 201, 5);

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let ranking = &structured["results"][0];
    assert_eq!(ranking["entity_count"], "0");
    assert_eq!(ranking["coverage"]["window_rows"], "2");
    assert_eq!(ranking["coverage"]["state"], "no_data");
}

#[test]
fn overview_names_the_layout_that_lacks_the_requested_fields() {
    let mut fixture = Fixture::new();
    // The fixture records the ossc layout; slow_log_calls exists only in
    // the vadv layout of the same logical section.
    fixture.append_plan_snapshots(&[(100, 7, 5, 1.5), (200, 7, 9, 2.5)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("pg_store_plans", &["slow_log_calls"], 100, 201, 5);

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let ranking = &structured["results"][0];
    assert_eq!(ranking["entity_count"], "0");
    assert_eq!(ranking["coverage"]["window_rows"], "0");
}

#[test]
fn overview_window_outside_the_recorded_range_is_no_data() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = overview_arguments("os_process", &["rmem_kb"], 1000, 1100, 5);

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let ranking = &structured["results"][0];
    assert_eq!(ranking["entity_count"], "0");
    assert_eq!(ranking["coverage"]["state"], "no_data");
    assert_eq!(ranking["coverage"]["window_rows"], "0");
}

#[test]
fn tools_reject_malformed_arguments_without_panicking() {
    use super::catalog::{
        FIND_EVENTS_TOOL, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
        FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL, FIND_POSTGRESQL_STATEMENTS_TOOL,
        FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL, FIND_PROCESSES_TOOL,
        GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL,
    };
    use super::{events, overview, postgresql, processes, row_detail};

    type Handler = fn(
        &Config,
        serde_json::Map<String, serde_json::Value>,
        &dyn Fn() -> bool,
    ) -> rmcp::model::CallToolResult;

    let config = test_config(std::env::temp_dir());
    let handlers: [(&str, Handler); 11] = [
        (OVERVIEW_TOOL, overview::call),
        (FIND_POSTGRESQL_TABLES_TOOL, postgresql::call_tables),
        (FIND_POSTGRESQL_ACTIVITY_TOOL, postgresql::call_activity),
        (FIND_POSTGRESQL_LOCKS_TOOL, postgresql::call_locks),
        (FIND_POSTGRESQL_VACUUM_TOOL, postgresql::call_vacuum),
        (FIND_POSTGRESQL_DATABASES_TOOL, postgresql::call_databases),
        (FIND_POSTGRESQL_STATEMENTS_TOOL, postgresql::call_statements),
        (FIND_POSTGRESQL_PLANS_TOOL, postgresql::call_plans),
        (FIND_PROCESSES_TOOL, processes::call),
        (GET_ROW_DETAIL_TOOL, row_detail::call),
        (FIND_EVENTS_TOOL, events::call),
    ];
    for (tool, handler) in handlers {
        let result = handler(&config, serde_json::Map::new(), &|| false);
        assert_eq!(
            result.is_error,
            Some(true),
            "{tool} accepted malformed arguments"
        );
    }
}

#[test]
fn overview_enforces_top_boundaries_and_defaults_to_twenty_five() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    for (top, returned) in [(1, 1), (500, 5)] {
        let result = crate::mcp::overview::call(
            &config,
            overview_arguments("os_process", &["rmem_kb"], 100, 400, top),
            &|| false,
        );
        assert_eq!(result.is_error, Some(false));
        let ranking = &result.structured_content.expect("structured content")["results"][0];
        assert_eq!(ranking["ranking"]["top"], top);
        assert_eq!(ranking["entities"].as_array().map(Vec::len), Some(returned));
    }

    let omitted = json!({
        "from": 100,
        "to": 400,
        "rankings": [{"section": "os_process", "fields": ["rmem_kb"]}],
    })
    .as_object()
    .expect("object")
    .clone();
    let omitted = crate::mcp::overview::call(&config, omitted, &|| false);
    assert_eq!(omitted.is_error, Some(false));
    assert_eq!(
        omitted.structured_content.expect("structured content")["results"][0]["ranking"]["top"],
        25
    );

    for top in [0, 501] {
        let rejected = crate::mcp::overview::call(
            &config,
            overview_arguments("os_process", &["rmem_kb"], 100, 400, top),
            &|| false,
        );
        assert_eq!(rejected.is_error, Some(true));
        assert_eq!(
            rejected.structured_content.expect("structured error")["ranking_index"],
            0
        );
    }
}

#[test]
fn overview_rejects_a_public_label_selector() {
    let config = test_config(std::env::temp_dir());
    let arguments = json!({
        "from": 100,
        "to": 400,
        "rankings": [{
            "section": "os_process",
            "fields": ["rmem_kb"],
            "labels": ["comm"],
        }],
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.expect("structured error")["ranking_index"],
        0
    );
}

#[test]
fn overview_reports_the_index_for_each_invalid_top_shape() {
    let config = test_config(std::env::temp_dir());
    for top in [json!(-1), json!(1.5), json!("25")] {
        let arguments = json!({
            "from": 100,
            "to": 400,
            "rankings": [
                {"section": "os_process", "fields": ["rmem_kb"], "top": 1},
                {"section": "os_process", "fields": ["rmem_kb"], "top": top},
            ],
        })
        .as_object()
        .expect("object")
        .clone();
        let result = crate::mcp::overview::call(&config, arguments, &|| false);
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("structured error")["ranking_index"],
            1
        );
    }
}

#[test]
fn overview_unknown_section_and_expanded_field_return_source_indices() {
    let config = test_config(std::env::temp_dir());
    let unknown_section = json!({
        "from": 100,
        "to": 400,
        "rankings": [
            {"section": "os_mountinfo", "fields": ["total_bytes"], "top": 1},
            {"section": "missing", "fields": ["total_bytes"], "top": 1},
        ],
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::overview::call(&config, unknown_section, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(message.contains("rankings[1]"), "{message}");
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["ranking_index"], 1);
    assert!(
        structured["valid_options"]
            .as_array()
            .expect("section options")
            .contains(&json!("os_process"))
    );

    let unknown_field = json!({
        "from": 100,
        "to": 400,
        "rankings": [
            {"section": "os_mountinfo", "fields": ["total_bytes"], "top": 1},
            {"section": "os_mountinfo", "fields": ["total_bytes", "missing"], "top": 1},
        ],
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::overview::call(&config, unknown_field, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(message.contains("rankings[1]"), "{message}");
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["ranking_index"], 1);
    assert_eq!(
        structured["valid_options"],
        json!([
            "total_bytes",
            "free_bytes",
            "total_inodes",
            "available_inodes",
        ])
    );
}

#[test]
fn overview_request_budget_is_exact_and_names_the_crossing_item() {
    const MAX: usize = 64 * 1024;
    let config = test_config(std::env::temp_dir());
    let arguments_at = |target: usize| {
        let mut value = json!({
            "from": 100,
            "to": 400,
            "rankings": [{
                "section": "os_process",
                "fields": [""],
                "top": 1,
            }],
        });
        let base = serde_json::to_vec(&value).expect("measure base").len();
        value["rankings"][0]["fields"][0] = json!("x".repeat(target - base));
        assert_eq!(serde_json::to_vec(&value).expect("measure").len(), target);
        value.as_object().expect("object").clone()
    };

    let at_limit = crate::mcp::overview::call(&config, arguments_at(MAX), &|| false);
    assert_eq!(at_limit.is_error, Some(true));
    let at_message = at_limit.content[0].as_text().expect("text").text.clone();
    assert!(!at_message.contains("arguments exceed"), "{at_message}");
    assert!(at_message.contains("rankings[0]"), "{at_message}");

    let over_limit = crate::mcp::overview::call(&config, arguments_at(MAX + 1), &|| false);
    assert_eq!(over_limit.is_error, Some(true));
    let over_message = over_limit.content[0].as_text().expect("text").text.clone();
    assert!(over_message.contains("arguments exceed"), "{over_message}");
    assert!(over_message.contains("rankings[0]"), "{over_message}");
    assert_eq!(
        over_limit.structured_content.expect("structured error")["ranking_index"],
        0
    );

    let long = "x".repeat(30_000);
    let crossing = json!({
        "from": 100,
        "to": 400,
        "rankings": [
            {"section": long, "fields": ["rmem_kb"], "top": 1},
            {"section": long, "fields": ["rmem_kb"], "top": 1},
            {"section": long, "fields": ["rmem_kb"], "top": 1},
        ],
    })
    .as_object()
    .expect("object")
    .clone();
    let crossing = crate::mcp::overview::call(&config, crossing, &|| false);
    assert_eq!(crossing.is_error, Some(true));
    let crossing_message = crossing.content[0].as_text().expect("text").text.clone();
    assert!(
        crossing_message.contains("rankings[2]"),
        "{crossing_message}"
    );
    assert_eq!(
        crossing.structured_content.expect("structured error")["ranking_index"],
        2
    );
}

#[tokio::test]
async fn overview_field_expansion_runs_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_rank_metrics",
            "arguments": {
                "from": 100,
                "to": 400,
                "rankings": [{
                    "section": "os_process",
                    "fields": ["rmem_kb", "num_threads"],
                    "top": 2,
                }],
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let results = decoded["result"]["structuredContent"]["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["ranking"]["fields"], json!(["rmem_kb"]));
    assert_eq!(results[0]["unit"], "kibibytes");
    assert_eq!(results[1]["ranking"]["fields"], json!(["num_threads"]));
    assert_eq!(results[1]["unit"], "count");
    let ranking = &results[0];
    let entities = ranking["entities"].as_array().expect("entities array");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["total"], 50.0);
    assert_eq!(entities[0]["identity"]["pid"], 101);
    assert_eq!(entities[1]["total"], 45.0);
    assert_eq!(ranking["totals_total"], 50.0);
    assert_eq!(ranking["others_total"], 30.0);
    assert_eq!(ranking["entity_count"], "5");
}

#[test]
fn find_postgresql_tables_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 0, "db", "public", "alpha"),
        (200, 1, 11, 30, "db", "public", "alpha"),
        (100, 1, 12, 0, "db", "public", "beta"),
        (200, 1, 12, 15, "db", "public", "beta"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "sort": {"field": "seq_scan", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["relname"], "alpha");
    assert_eq!(rows[0]["datname"], "db");
    assert_eq!(rows[1]["relname"], "beta");
    assert_eq!(structured["truncated"], false);

    let filtered_arguments = serde_json::json!({
        "group": "object",
        "filters": [{"field": "table_name", "op": "eq", "value": "alpha"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_tables(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["relname"], "alpha");
}

#[test]
fn find_postgresql_tables_rejects_a_limit_above_the_snapshot_page_size_cap() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "limit": 4_000_000_000_u64,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("5000") || message.contains("limit"),
        "error must name the limit cap: {message}"
    );
}

#[test]
fn find_postgresql_indexes_returns_keyed_rows_with_indexrelname() {
    let mut fixture = Fixture::new();
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            21,
            0,
            "db",
            "public",
            "alpha",
            "alpha_pkey",
            "CREATE UNIQUE INDEX alpha_pkey ON alpha USING btree (id)",
        ),
        (
            200,
            1,
            21,
            20,
            "db",
            "public",
            "alpha",
            "alpha_pkey",
            "CREATE UNIQUE INDEX alpha_pkey ON alpha USING btree (id)",
        ),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_indexes(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["indexrelname"], "alpha_pkey");
    assert_eq!(rows[0]["relname"], "alpha");
    assert_eq!(structured["truncated"], false);
    assert_no_storage_coordinate_keys(&structured);
}

#[test]
fn find_postgresql_activity_ranks_and_filters() {
    // `append_postgres_health(2)` records active PIDs 0 and 1 and idle PID
    // 10,000.
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(2);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "pid", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["pid"], 10_000);
    assert_eq!(rows[0]["state"], "idle");
    assert_detail_ref(&rows[0], "pg_stat_activity", &["query"]);
    assert_detail_roundtrip(&config, &rows[0], "pid", &json!(10_000));
    assert_eq!(structured["truncated"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "state", "op": "eq", "value": "idle"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_activity(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 10_000);
}

#[tokio::test]
async fn find_postgresql_activity_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(2);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_postgresql_activity",
            "arguments": {
                "filters": [],
                "sort": {"field": "pid", "direction": "desc"},
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    assert_wire_json_content_matches_structured(&decoded);
    let rows = decoded["result"]["structuredContent"]["rows"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["pid"], 10_000);
    assert_eq!(rows[0]["state"], "idle");
    assert_eq!(decoded["result"]["structuredContent"]["truncated"], false);
}

#[test]
fn find_postgresql_locks_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_lock_rows(&[
        (100, 701, "active", "RowExclusiveLock"),
        (100, 702, "active", "AccessShareLock"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "pid", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_locks(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 702);
    assert_eq!(rows[1]["pid"], 701);
    assert_detail_ref(&rows[0], "pg_locks", &["query"]);
    assert_detail_roundtrip(&config, &rows[0], "pid", &json!(702));
    assert_eq!(structured["truncated"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "lock_mode", "op": "eq", "value": "AccessShareLock"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_locks(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 702);
}

#[test]
fn find_postgresql_vacuum_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_vacuum_rows(&[
        (100, 501, "scanning heap", 1_000, 500, 400),
        (100, 502, "vacuuming indexes", 2_000, 2_000, 1_800),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "heap_blks_scanned", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_vacuum(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 502);
    assert_eq!(rows[1]["pid"], 501);
    assert_detail_ref(&rows[0], "pg_stat_progress_vacuum", &[]);
    assert_detail_roundtrip(&config, &rows[0], "pid", &json!(502));
    assert_eq!(structured["truncated"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "phase", "op": "eq", "value": "scanning heap"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_vacuum(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 501);
}

#[test]
fn find_postgresql_databases_ranks_and_filters() {
    // Two samples for one database produce a positive deadlock rate.
    let mut fixture = Fixture::new();
    fixture.append_postgres_database_snapshots(&[(100, 100, 10, 0), (200, 180, 30, 7)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "datid", "direction": "asc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_databases(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["datid"], 73);
    assert_eq!(rows[0]["datname"], "db");
    assert_detail_ref(&rows[0], "pg_stat_database", &[]);
    assert_detail_roundtrip(&config, &rows[0], "datid", &json!(73));
    assert_eq!(structured["truncated"], false);

    let matching_arguments = serde_json::json!({
        "filters": [{"field": "deadlocks", "op": "gt", "value": 0}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let matching = crate::mcp::postgresql::call_databases(&config, matching_arguments, &|| false);
    assert_eq!(matching.is_error, Some(false));
    let matching_rows = matching.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(matching_rows.len(), 1);

    let below_threshold_arguments = serde_json::json!({
        "filters": [{"field": "deadlocks", "op": "gt", "value": 100}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let below_threshold =
        crate::mcp::postgresql::call_databases(&config, below_threshold_arguments, &|| false);
    assert_eq!(below_threshold.is_error, Some(false));
    let below_threshold_rows = below_threshold
        .structured_content
        .expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert!(below_threshold_rows.is_empty());
}

/// Checks a JSON number within `1e-9`; composed ratios divide previously
/// rounded rates, so exact `f64` equality is not stable.
fn assert_close(actual: &serde_json::Value, expected: f64) {
    let actual = actual
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {actual:?}"));
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn find_postgresql_statements_computes_derived_ratio_fields() {
    // Expected fields use the `ts=200 - ts=100` deltas from
    // `append_ranked_statements`.
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array").clone();
    assert_eq!(rows.len(), 3);

    let row = |queryid: &str| {
        rows.iter()
            .find(|row| row["queryid"] == queryid)
            .unwrap_or_else(|| panic!("no row for queryid {queryid}"))
    };

    let one = row("1");
    assert_close(&one["derived_mean_exec_ms_per_call"], 10.0);
    assert_close(&one["derived_rows_per_call"], 10.0);
    assert_close(&one["derived_blocks_per_call"], 10.0);
    assert_close(&one["derived_hit_fraction"], 0.8);
    assert_close(&one["derived_wal_per_call"], 10.0);
    assert_close(&one["derived_plan_time_fraction"], 0.5);
    assert_close(&one["derived_cv"], 0.9);
    assert_detail_ref(one, "pg_stat_statements", &["query"]);
    assert_detail_roundtrip(&config, one, "queryid", &json!("1"));

    let two = row("2");
    assert_close(&two["derived_mean_exec_ms_per_call"], 15.0);
    assert_close(&two["derived_rows_per_call"], 30.0);
    assert_close(&two["derived_blocks_per_call"], 25.0);
    assert_close(&two["derived_hit_fraction"], 0.9);
    assert_close(&two["derived_wal_per_call"], 30.0);
    assert_close(&two["derived_plan_time_fraction"], 40.0 / 70.0);
    assert_close(&two["derived_cv"], 2.0);

    let three = row("3");
    assert_close(&three["derived_mean_exec_ms_per_call"], 5.0);
    assert_close(&three["derived_rows_per_call"], 1.0);
    assert_close(&three["derived_blocks_per_call"], 3.0);
    assert_close(&three["derived_hit_fraction"], 1.0 / 3.0);
    assert_close(&three["derived_wal_per_call"], 5.0);
    assert_close(&three["derived_plan_time_fraction"], 1.0 / 6.0);
    assert_close(&three["derived_cv"], 0.2);
}

#[test]
fn find_postgresql_statements_nulls_derived_fields_without_a_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 100.0)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    for field in [
        "derived_mean_exec_ms_per_call",
        "derived_rows_per_call",
        "derived_blocks_per_call",
        "derived_hit_fraction",
        "derived_wal_per_call",
        "derived_plan_time_fraction",
        "derived_cv",
    ] {
        assert_eq!(
            row[field],
            serde_json::Value::Null,
            "{field} must be null without a predecessor snapshot"
        );
    }
}

#[test]
fn find_postgresql_plans_computes_derived_ratio_fields() {
    // The OSSC `pg_store_plans` layout lacks `wal_bytes` and
    // `total_plan_time`, so both derived fields are null. `mean_time` and
    // `stddev_time` are gauges; `derived_cv` needs no predecessor.
    let mut fixture = Fixture::new();
    fixture.append_ranked_plans();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array").clone();
    assert_eq!(rows.len(), 3);

    let row = |queryid: &str| {
        rows.iter()
            .find(|row| row["queryid"] == queryid)
            .unwrap_or_else(|| panic!("no row for queryid {queryid}"))
    };

    let one = row("1");
    assert_detail_ref(one, "pg_store_plans", &["plan"]);
    assert_detail_roundtrip(&config, one, "queryid", &json!("1"));
    assert_close(&one["derived_mean_exec_ms_per_call"], 10.0);
    assert_close(&one["derived_rows_per_call"], 10.0);
    assert_close(&one["derived_blocks_per_call"], 10.0);
    assert_close(&one["derived_hit_fraction"], 0.8);
    assert_eq!(one["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(one["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&one["derived_cv"], 2.2 / 24.9);

    let two = row("2");
    assert_close(&two["derived_mean_exec_ms_per_call"], 15.0);
    assert_close(&two["derived_rows_per_call"], 30.0);
    assert_close(&two["derived_blocks_per_call"], 25.0);
    assert_close(&two["derived_hit_fraction"], 0.9);
    assert_eq!(two["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(two["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&two["derived_cv"], 2.2 / 24.9);

    let three = row("3");
    assert_close(&three["derived_mean_exec_ms_per_call"], 5.0);
    assert_close(&three["derived_rows_per_call"], 1.0);
    assert_close(&three["derived_blocks_per_call"], 3.0);
    assert_close(&three["derived_hit_fraction"], 1.0 / 3.0);
    assert_eq!(three["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(three["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&three["derived_cv"], 2.2 / 24.9);
}

#[test]
fn find_postgresql_plans_nulls_rate_derived_fields_without_a_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_plan_snapshots(&[(100, 1, 10, 100.0)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    // Rate-dependent fields need a predecessor.
    for field in [
        "derived_mean_exec_ms_per_call",
        "derived_rows_per_call",
        "derived_blocks_per_call",
        "derived_hit_fraction",
    ] {
        assert_eq!(
            row[field],
            serde_json::Value::Null,
            "{field} must be null without a predecessor snapshot"
        );
    }
    // Absent on this layout regardless of predecessor.
    assert_eq!(row["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(row["derived_plan_time_fraction"], serde_json::Value::Null);
    // Gauge-only: computable even without a predecessor.
    assert_close(&row["derived_cv"], 2.2 / 24.9);
}

#[test]
fn find_postgresql_plans_computes_plan_time_fraction_on_the_vadv_layout() {
    // Only the vadv layout (`1_004_001`) records `total_plan_time`; this
    // fixture makes `derived_plan_time_fraction` non-null.
    let mut fixture = Fixture::new();
    fixture.append_ranked_vadv_plans();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    // total_plan_time=25.0, total_time=100.0 => 25 / (25 + 100) = 0.2.
    assert_close(&row["derived_plan_time_fraction"], 0.2);
}

#[test]
fn find_processes_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "rmem_kb", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::processes::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 101);
    assert_eq!(rows[1]["pid"], 102);
    assert_detail_ref(&rows[0], "os_process", &["cmdline"]);
    assert_detail_roundtrip(&config, &rows[0], "pid", &json!(101));
    assert_eq!(structured["truncated"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 102}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::processes::call(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 102);
}

#[test]
fn find_processes_rejects_a_limit_above_the_snapshot_page_size_cap() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "filters": [],
        "limit": 4_000_000_000_u64,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::processes::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("5000") || message.contains("limit"),
        "error must name the limit cap: {message}"
    );
}

#[tokio::test]
async fn find_processes_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_processes",
            "arguments": {
                "filters": [],
                "sort": {"field": "rmem_kb", "direction": "desc"},
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    assert_wire_json_content_matches_structured(&decoded);
    let rows = decoded["result"]["structuredContent"]["rows"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 101);
    assert_eq!(rows[1]["pid"], 102);
    assert_eq!(decoded["result"]["structuredContent"]["truncated"], false);
}

#[test]
fn get_row_detail_agrees_with_find_processes_for_the_same_physical_row() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());

    let listing_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 101}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::processes::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();
    assert_detail_ref(&listing_row, "os_process", &["cmdline"]);

    let detail = crate::mcp::row_detail::call(&config, detail_arguments(&listing_row), &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["pid"], listing_row["pid"]);
    assert_eq!(detail_row["rmem_kb"], listing_row["rmem_kb"]);
    assert_no_storage_coordinate_keys(&detail_row);
}

#[test]
fn get_row_detail_chains_directly_from_a_find_processes_ref() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());

    let listing_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 102}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::processes::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();
    assert_detail_ref(&listing_row, "os_process", &["cmdline"]);

    let detail = crate::mcp::row_detail::call(&config, detail_arguments(&listing_row), &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["pid"], listing_row["pid"]);
    assert_eq!(detail_row["rmem_kb"], listing_row["rmem_kb"]);
    assert_no_storage_coordinate_keys(&detail_row);
    assert_eq!(
        detail_row["cmdline"],
        json!({
            "stored_text": "beta",
            "full_len": "4",
            "truncated": false,
            "sha256": null,
        })
    );
}

#[test]
fn get_row_detail_preserves_truncated_blob_text_facts_in_the_stable_shape() {
    let cmdline = "x".repeat(kronika_format::DEFAULT_TRUNCATE_LIMIT + 17);
    let mut fixture = Fixture::new();
    fixture.append_process_cmdline(100, 101, &cmdline);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let found = crate::mcp::processes::call(
        &config,
        json!({"filters": [], "limit": 1})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(found.is_error, Some(false));
    let found = &found.structured_content.expect("finder result")["rows"][0];
    assert_detail_ref(found, "os_process", &["cmdline"]);
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(found), &|| false);
    assert_eq!(detail.is_error, Some(false));
    let detail = detail.structured_content.expect("detail row");
    let text = detail["cmdline"].as_object().expect("stable text object");
    assert_eq!(text.len(), 4);
    assert_eq!(
        text["stored_text"].as_str().map(str::len),
        Some(kronika_format::DEFAULT_TRUNCATE_LIMIT)
    );
    assert_eq!(text["full_len"], cmdline.len().to_string());
    assert_eq!(text["truncated"], true);
    assert_eq!(text["sha256"].as_str().map(str::len), Some(64));
    assert!(!text.contains_key("representation"));
}

#[test]
fn get_row_detail_accepts_relation_overview_but_aggregates_have_no_ref() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let overview = crate::mcp::overview::call(
        &config,
        overview_arguments("pg_stat_user_tables", &["main_fork_bytes"], 100, 101, 1),
        &|| false,
    );
    assert_eq!(overview.is_error, Some(false));
    let overview = overview.structured_content.expect("Overview");
    let entity = &overview["results"][0]["entities"][0];
    assert_eq!(entity["identity"]["datid"], 1);
    assert_eq!(entity["identity"]["relid"], 11);
    assert_detail_ref(entity, "pg_stat_user_tables", &[]);
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(entity), &|| false);
    assert_eq!(detail.is_error, Some(false));
    assert_eq!(
        detail.structured_content.expect("detail")["relname"],
        "alpha"
    );

    let aggregate = crate::mcp::postgresql::call_tables(
        &config,
        json!({"group": "object", "limit": 10})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(aggregate.is_error, Some(false));
    let aggregate = aggregate.structured_content.expect("aggregate finder");
    assert!(aggregate["rows"][0].get("detail_ref").is_none());
    assert_no_storage_coordinate_keys(&aggregate);
}

#[test]
fn get_row_detail_rejects_the_old_structured_locator_input() {
    let config = test_config(std::env::temp_dir());
    let arguments = json!({
        "section": "os_process",
        "segment_id": "7",
        "at": "100",
        "type_id": "1100001",
        "row_ordinal": "0",
        "identity": {"pid": "101"},
    })
    .as_object()
    .expect("arguments")
    .clone();

    let result = crate::mcp::row_detail::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = &result.content[0].as_text().expect("error").text;
    assert_eq!(
        message,
        "invalid arguments: pass only one copied detail_ref string"
    );
}

#[test]
fn get_row_detail_rejects_a_malformed_ref_before_opening_storage() {
    let directory = tempfile::tempdir().expect("temporary parent");
    let config = test_config(directory.path().join("missing-root"));
    let arguments = json!({"detail_ref": "not+base64"})
        .as_object()
        .expect("arguments")
        .clone();

    let result = crate::mcp::row_detail::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.content[0].as_text().expect("error").text,
        "invalid detail_ref"
    );
    assert_eq!(
        result.structured_content,
        Some(json!({
            "record": "error",
            "message": "invalid detail_ref",
        }))
    );
}

#[test]
fn find_events_returns_structural_fields_with_one_opaque_ref() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["occurrences"]
        .as_array()
        .expect("occurrences array");
    assert_eq!(rows.len(), 2);
    assert_eq!(structured["representation"], "occurrences");
    assert_eq!(structured["truncated"], false);
    for row in rows {
        assert_eq!(row["source"], "pg_log_errors");
        assert_eq!(row["source_file"], "fixture");
        assert_eq!(row["pattern"], "fixture");
        assert!(row.get("sample").is_none());
        assert_detail_ref(row, "pg_log_errors", &["sample"]);
        assert_eq!(row["category"], 8);
    }
    let detail = crate::mcp::row_detail::call(&config, detail_arguments(&rows[0]), &|| false);
    assert_eq!(detail.is_error, Some(false));
    let detail = detail.structured_content.expect("occurrence detail");
    assert_eq!(detail["sample"]["stored_text"], "fixture");
    assert_no_storage_coordinate_keys(&detail);
}

#[test]
fn find_events_default_and_explicit_groups_are_identical() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let base = json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    });
    let default =
        crate::mcp::events::call(&config, base.as_object().expect("object").clone(), &|| {
            false
        });
    let mut explicit = base.as_object().expect("object").clone();
    explicit.insert("representation".to_owned(), json!("groups"));
    let explicit = crate::mcp::events::call(&config, explicit, &|| false);

    assert_eq!(default.is_error, Some(false));
    assert_eq!(explicit.is_error, Some(false));
    assert_eq!(default.structured_content, explicit.structured_content);
    let structured = default.structured_content.expect("structured groups");
    assert_eq!(structured["representation"], "groups");
    assert_eq!(structured["groups"].as_array().expect("groups").len(), 1);
    assert!(structured.get("occurrences").is_none());
    let group = &structured["groups"][0];
    assert_eq!(group["label"], "fixture");
    assert!(group.get("text").is_none());
    assert!(group.get("rows").is_none());
    assert_detail_ref(group, "pg_log_errors", &[]);
    let group_detail = crate::mcp::row_detail::call(&config, detail_arguments(group), &|| false);
    assert_eq!(group_detail.is_error, Some(false));
    assert_eq!(
        group_detail.structured_content.expect("group detail")["sample"]["stored_text"],
        "fixture"
    );

    let http = streamed(fixture.prepare(
        "/api/events?from=0&to=1000&source=pg_log_errors&representation=groups&limit=10",
        None,
    ));
    assert_eq!(structured["representation"], http[0]["representation"]);
    assert_eq!(structured["truncated"], http[0]["truncated"]);
    let http_groups = &http[1..];
    let mcp_groups = structured["groups"].as_array().expect("MCP groups");
    assert_eq!(mcp_groups.len(), http_groups.len());
    for (mcp, http) in mcp_groups.iter().zip(http_groups) {
        let mut mcp = mcp.as_object().expect("MCP group").clone();
        let mcp_ref = mcp.remove("detail_ref").expect("MCP detail ref");
        let mut http = http.as_object().expect("HTTP group").clone();
        assert_eq!(http.remove("record"), Some(json!("event_group")));
        let http_ref = http.remove("detail_ref").expect("HTTP detail ref");
        assert_eq!(mcp_ref, http_ref);
        assert_eq!(mcp, http, "HTTP and MCP keep the same semantic fields");
        assert_no_storage_coordinate_keys(&serde_json::Value::Object(http));
    }
}

#[test]
fn find_events_labels_the_numeric_severity_and_category_codes() {
    // `severity` and `category` are unordered Kronika codes.
    // `append_log_error` records 0 (`error`) and 8 (`auth`).
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["occurrences"]
        .as_array()
        .expect("occurrences array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["severity"], 0, "raw numeric code stays untouched");
    assert_eq!(row["severity_label"], "error");
    assert_eq!(row["category"], 8);
    assert_eq!(row["category_label"], "auth");
}

#[test]
fn find_events_sorts_returned_candidates_by_timestamp() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_pgbouncer_event(150);
    fixture.append_log_error(300);
    fixture.append_pgbouncer_event(250);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors", "pgbouncer_events"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 3,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["occurrences"]
        .as_array()
        .expect("occurrences array");
    assert_eq!(rows.len(), 3, "4 rows exist in the window but limit is 3");
    assert_eq!(structured["truncated"], true);
    assert!(structured.get("has_more").is_none());
    assert!(structured.get("next_from").is_none());
    let ordered_ats: Vec<i64> = rows
        .iter()
        .map(|row| {
            assert_detail_ref(row, row["source"].as_str().expect("source"), &[]);
            let detail = crate::mcp::row_detail::call(&config, detail_arguments(row), &|| false);
            assert_eq!(detail.is_error, Some(false));
            detail.structured_content.expect("event detail")["at"]
                .as_str()
                .expect("at is a decimal string")
                .parse()
                .expect("at parses")
        })
        .collect();
    assert_eq!(
        ordered_ats,
        vec![100, 150, 250],
        "retained fixture rows are sorted by timestamp"
    );
    let sources: Vec<&str> = rows
        .iter()
        .map(|row| row["source"].as_str().expect("source"))
        .collect();
    assert_eq!(
        sources,
        vec!["pg_log_errors", "pgbouncer_events", "pgbouncer_events"]
    );
    for row in rows
        .iter()
        .filter(|row| row["source"] == "pgbouncer_events")
    {
        assert!(row.get("text").is_none());
        assert_detail_ref(row, "pgbouncer_events", &["text"]);
    }
}

#[test]
fn find_events_rejects_a_3_600_000_000_microsecond_endpoint_difference() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 3_600_000_001_i64,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("3600000000") || message.contains("one hour"),
        "error must name the window limit: {message}"
    );
}

#[test]
fn find_events_accepts_a_window_of_exactly_one_hour() {
    // "At most one hour" includes one hour itself: the schema promises the
    // span, so the boundary must not be off by one microsecond.
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 3_600_000_000_i64,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
}

#[test]
fn a_quantity_filter_refusal_lists_the_accepted_operators() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [{"field": "rss", "op": "eq", "value": 1}],
        "limit": 5,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::processes::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["record"], "error");
    assert_eq!(structured["valid_options"], serde_json::json!(["gt", "lt"]));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("it accepts gt, lt"),
        "the refusal names the alternatives: {message}"
    );
}

#[test]
fn an_unknown_filter_operator_lists_valid_options_before_tag_decoding() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "filters": [{"field": "rss", "op": "approximately", "value": 1}],
        "limit": 5,
    })
    .as_object()
    .expect("object")
    .clone();
    let request = rmcp::model::CallToolRequestParams::new(crate::mcp::catalog::FIND_PROCESSES_TOOL)
        .with_arguments(arguments);
    let result =
        crate::mcp::dispatch::dispatch(&config, request, &|| false).expect("known tool dispatches");

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["record"], "error");
    assert_eq!(structured["valid_options"], serde_json::json!(["gt", "lt"]));
}

#[test]
fn every_finder_uses_the_exact_shared_argument_budget() {
    const MAX: usize = crate::route::MAX_QUERY_BYTES;
    const FINDERS: [&str; 9] = [
        crate::mcp::catalog::FIND_PROCESSES_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_TABLES_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_INDEXES_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_ACTIVITY_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_LOCKS_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_VACUUM_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_DATABASES_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_STATEMENTS_TOOL,
        crate::mcp::catalog::FIND_POSTGRESQL_PLANS_TOOL,
    ];
    let arguments_at = |target: usize| {
        let mut value = json!({"limit": 1, "padding": ""});
        let base = serde_json::to_vec(&value).expect("measure base").len();
        value["padding"] = json!("x".repeat(target - base));
        assert_eq!(serde_json::to_vec(&value).expect("measure").len(), target);
        value.as_object().expect("object").clone()
    };
    let config = test_config(std::env::temp_dir());

    let at_limit =
        rmcp::model::CallToolRequestParams::new(FINDERS[0]).with_arguments(arguments_at(MAX));
    let at_limit = crate::mcp::dispatch::dispatch(&config, at_limit, &|| false)
        .expect("known tool dispatches");
    let at_message = at_limit.content[0].as_text().expect("text").text.clone();
    assert!(!at_message.contains("arguments exceed"), "{at_message}");

    for tool in FINDERS {
        let over_limit =
            rmcp::model::CallToolRequestParams::new(tool).with_arguments(arguments_at(MAX + 1));
        let over_limit = crate::mcp::dispatch::dispatch(&config, over_limit, &|| false)
            .expect("known tool dispatches");
        let message = over_limit.content[0].as_text().expect("text").text.clone();
        assert!(
            message.contains("arguments exceed 65536"),
            "{tool}: {message}"
        );
    }
}

#[test]
fn rejected_tool_arguments_name_the_usage() {
    let config = test_config(std::env::temp_dir());
    let result =
        crate::mcp::postgresql::call_statements(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("kronika_find_postgresql_statements") && message.contains("Usage:"),
        "{message}"
    );
}

#[test]
fn get_context_narrows_to_one_section_and_lists_names_on_a_miss() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "section": "os_process" })
        .as_object()
        .expect("object")
        .clone();
    let result = crate::mcp::context::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let sections = structured["sections"].as_array().expect("sections");
    assert!(!sections.is_empty());
    assert!(
        sections
            .iter()
            .all(|section| section["logical_name"] == "os_process")
    );

    let arguments = serde_json::json!({ "section": "not_recorded" })
        .as_object()
        .expect("object")
        .clone();
    let result = crate::mcp::context::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    let options = structured["valid_options"].as_array().expect("options");
    assert!(options.contains(&serde_json::json!("os_process")));
}

#[test]
fn find_events_rejects_an_unknown_source_name() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "sources": ["pg_log_made_up"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(
        structured["valid_options"],
        serde_json::json!([
            "pg_log_errors",
            "pg_log_checkpoints",
            "pg_log_autovacuum",
            "pg_log_slow_queries",
            "pg_log_lock_waits",
            "pg_log_lifecycle",
            "pgbouncer_events"
        ])
    );
}

#[test]
fn find_events_request_budget_is_exact() {
    const MAX: usize = crate::route::MAX_QUERY_BYTES;
    let config = test_config(std::env::temp_dir());
    let arguments_at = |target: usize| {
        let mut value = json!({
            "from": 100,
            "to": 400,
            "sources": [""],
            "limit": 1,
        });
        let base = serde_json::to_vec(&value).expect("measure base").len();
        value["sources"][0] = json!("x".repeat(target - base));
        assert_eq!(serde_json::to_vec(&value).expect("measure").len(), target);
        value.as_object().expect("object").clone()
    };

    let at_limit = crate::mcp::events::call(&config, arguments_at(MAX), &|| false);
    assert_eq!(at_limit.is_error, Some(true));
    let at_message = at_limit.content[0].as_text().expect("text").text.clone();
    assert!(!at_message.contains("arguments exceed"), "{at_message}");

    let over_limit = crate::mcp::events::call(&config, arguments_at(MAX + 1), &|| false);
    assert_eq!(over_limit.is_error, Some(true));
    let over_message = over_limit.content[0].as_text().expect("text").text.clone();
    assert!(
        over_message.contains("arguments exceed 65536"),
        "{over_message}"
    );
}

#[test]
fn get_row_detail_uses_an_opaque_ref_for_the_complete_identity() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();
    let listing = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    let one = rows
        .iter()
        .find(|row| row["queryid"] == "1")
        .expect("queryid 1");
    let two = rows
        .iter()
        .find(|row| row["queryid"] == "2")
        .expect("queryid 2");
    assert_detail_ref(one, "pg_stat_statements", &["query"]);
    assert_detail_ref(two, "pg_stat_statements", &["query"]);
    assert_ne!(one["detail_ref"], two["detail_ref"]);

    let same = crate::mcp::row_detail::call(&config, detail_arguments(one), &|| false);
    assert_eq!(same.is_error, Some(false));
    let same = same.structured_content.expect("structured content");
    assert_eq!(same["queryid"], "1");
    assert_no_storage_coordinate_keys(&same);

    let mut modified = one["detail_ref"].as_str().expect("detail_ref").to_owned();
    modified.push('A');
    let rejected = crate::mcp::row_detail::call(
        &config,
        json!({"detail_ref": modified})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    assert_eq!(rejected.is_error, Some(true));
    let message = &rejected.content[0].as_text().expect("text").text;
    assert!(message.contains("invalid detail_ref"), "{message}");
}

#[test]
fn get_row_detail_maps_cancelled_missing_and_non_unique_lookups_stably() {
    let mut source = Fixture::new();
    source.append_process_gauge_rows(&[(100, 101, 50, "source")]);
    source.finish();
    let source_config = test_config(source.root().to_path_buf());
    let found = crate::mcp::processes::call(
        &source_config,
        json!({"filters": [], "limit": 1})
            .as_object()
            .expect("arguments")
            .clone(),
        &|| false,
    );
    let row = &found.structured_content.expect("finder result")["rows"][0];
    let arguments = detail_arguments(row);

    let cancelled = crate::mcp::row_detail::call(&source_config, arguments.clone(), &|| true);
    assert_eq!(cancelled.is_error, Some(true));
    assert_eq!(
        cancelled.content[0].as_text().expect("error").text,
        "request cancelled"
    );
    assert_eq!(
        cancelled.structured_content,
        Some(json!({
            "record": "error",
            "message": "request cancelled",
        }))
    );

    let mut missing = Fixture::new();
    missing.append_process_gauge_rows(&[(100, 102, 40, "other")]);
    missing.finish();
    let missing = crate::mcp::row_detail::call(
        &test_config(missing.root().to_path_buf()),
        arguments.clone(),
        &|| false,
    );
    assert_eq!(missing.is_error, Some(true));
    assert_eq!(
        missing.content[0].as_text().expect("error").text,
        "detail_ref does not identify one recorded row"
    );

    let mut duplicate = Fixture::new();
    duplicate.append_process_gauge_rows(&[(100, 101, 50, "first"), (100, 101, 60, "second")]);
    duplicate.finish();
    let duplicate = crate::mcp::row_detail::call(
        &test_config(duplicate.root().to_path_buf()),
        arguments,
        &|| false,
    );
    assert_eq!(duplicate.is_error, Some(true));
    assert_eq!(
        duplicate.content[0].as_text().expect("error").text,
        "detail_ref does not identify one recorded row"
    );
}

#[test]
fn find_processes_refuses_to_emit_a_ref_for_a_non_unique_identity() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "first"), (100, 101, 60, "second")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = crate::mcp::processes::call(
        &config,
        json!({
            "filters": [{"field": "command", "op": "eq", "value": "first"}],
            "limit": 10,
        })
        .as_object()
        .expect("arguments")
        .clone(),
        &|| false,
    );

    assert_eq!(result.is_error, Some(true));
    let message = &result.content[0].as_text().expect("error").text;
    assert_eq!(message, "could not produce detail_ref");
    if let Some(body) = &result.structured_content {
        assert_no_detail_ref_property(body);
    }
    for forbidden in FORBIDDEN_COORDINATE_KEYS {
        assert!(!message.contains(forbidden), "error exposed {forbidden}");
    }
}

#[test]
fn get_row_detail_chains_directly_from_a_find_events_ref() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let listing_arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::events::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["occurrences"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();
    assert_detail_ref(&listing_row, "pg_log_errors", &["sample"]);

    let detail = crate::mcp::row_detail::call(&config, detail_arguments(&listing_row), &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["category"], listing_row["category"]);
    assert_no_storage_coordinate_keys(&detail_row);
    assert_eq!(detail_row["sample"]["stored_text"], "fixture");
    assert_eq!(
        detail_row["sample"].as_object().map(serde_json::Map::len),
        Some(4)
    );
}

#[test]
fn find_events_emits_refs_for_content_equivalent_occurrences() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("arguments")
    .clone();
    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let occurrences = result.structured_content.expect("finder result")["occurrences"]
        .as_array()
        .expect("occurrences")
        .clone();
    assert_eq!(occurrences.len(), 2);
    for occurrence in &occurrences {
        assert_detail_ref(occurrence, "pg_log_errors", &["sample"]);
    }
    let first = crate::mcp::row_detail::call(&config, detail_arguments(&occurrences[0]), &|| false);
    let second =
        crate::mcp::row_detail::call(&config, detail_arguments(&occurrences[1]), &|| false);
    assert_eq!(first.is_error, Some(false));
    assert_eq!(second.is_error, Some(false));
    assert_eq!(first.structured_content, second.structured_content);
}

#[test]
fn find_events_truncates_after_an_equivalent_retained_identity() {
    let mut fixture = Fixture::new();
    fixture.append_log_error_count(100, 1);
    fixture.append_log_error_count(100, 2);
    fixture.append_log_error_count(100, 1);
    let config = test_config(fixture.root().to_path_buf());
    let arguments = json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 1,
    })
    .as_object()
    .expect("arguments")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let body = result.structured_content.expect("finder result");
    assert_eq!(body["truncated"], true);
    let occurrences = body["occurrences"].as_array().expect("occurrences");
    assert_eq!(occurrences.len(), 1);
    assert_detail_ref(&occurrences[0], "pg_log_errors", &["sample"]);
}

#[test]
fn get_row_detail_labels_the_same_numeric_codes_find_events_does() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let listing_arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::events::call(&config, listing_arguments, &|| false);
    let listing_row =
        listing.structured_content.expect("structured content")["occurrences"][0].clone();

    let detail = crate::mcp::row_detail::call(&config, detail_arguments(&listing_row), &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["severity_label"], "error");
    assert_eq!(detail_row["category_label"], "auth");
    assert_eq!(detail_row["severity_label"], listing_row["severity_label"]);
    assert_eq!(detail_row["category_label"], listing_row["category_label"]);
}

#[tokio::test]
async fn find_events_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_events",
            "arguments": {
                "sources": ["pg_log_errors"],
                "from": 0,
                "to": 1_000,
                "representation": "occurrences",
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    assert_wire_json_content_matches_structured(&decoded);
    let rows = decoded["result"]["structuredContent"]["occurrences"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["source"], "pg_log_errors");
        assert_eq!(row["category"], 8);
        assert_detail_ref(row, "pg_log_errors", &["sample"]);
    }
    assert_eq!(decoded["result"]["structuredContent"]["truncated"], false);
    assert_eq!(
        decoded["result"]["structuredContent"]["representation"],
        "occurrences"
    );
}

#[test]
fn find_tables_uses_a_recent_section_sample_at_the_global_store_bound() {
    // Relations ride a slower cadence than the rest, so the newest segment
    // regularly has none — the tool must answer from the newest segment
    // that does, not fail with a paging error.
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish_and_continue(1_709_164_800_000_000 + 1_000);
    fixture.append_process_gauge_rows(&[(200, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["relname"], "alpha");
    assert!(structured.get("as_of").is_none());
}

#[test]
fn find_vacuum_reports_no_recorded_rows_instead_of_an_error() {
    // On a healthy host this is the tool's normal state: the section is
    // recorded only while a vacuum runs.
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_vacuum(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["rows"].as_array(), Some(&Vec::new()));
    assert_eq!(structured["truncated"], false);
    assert!(structured.get("as_of").is_none());
}

#[test]
fn unknown_sort_fields_are_rejected_not_ignored() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(1);
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let plain = serde_json::json!({
        "sort": {"field": "query_duration", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::postgresql::call_activity(&config, plain, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("no such sort field"),
        "unexpected message: {message}"
    );

    let relation = serde_json::json!({
        "group": "object",
        "sort": {"field": "not_a_field", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::postgresql::call_tables(&config, relation, &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn filter_eq_matches_the_whole_value_and_contains_a_substring() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(1);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let count_for = |op: &str, value: &str| {
        let arguments = serde_json::json!({
            "filters": [{"field": "state", "op": op, "value": value}],
            "limit": 10,
        })
        .as_object()
        .expect("object")
        .clone();
        let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);
        assert_eq!(result.is_error, Some(false));
        result.structured_content.expect("structured content")["rows"]
            .as_array()
            .expect("rows array")
            .len()
    };

    assert_eq!(count_for("eq", "idle"), 1);
    assert_eq!(count_for("eq", "idl"), 0);
    assert_eq!(count_for("contains", "idl"), 1);
}

#[test]
fn more_than_eight_filters_are_rejected() {
    let config = test_config(std::env::temp_dir());
    let filters: Vec<serde_json::Value> = (0..9)
        .map(|_| serde_json::json!({"field": "state", "op": "eq", "value": "idle"}))
        .collect();
    let arguments = serde_json::json!({ "filters": filters, "limit": 10 })
        .as_object()
        .expect("object")
        .clone();
    let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("too many filters"),
        "unexpected message: {message}"
    );
}

#[test]
fn overview_preserves_field_order_duplicates_and_independent_top_without_summing() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "from": 100,
        "to": 201,
        "rankings": [
            {
                "section": "pg_stat_statements",
                "fields": ["local_blks_hit", "calls", "local_blks_hit"],
                "top": 1,
            },
            {
                "section": "pg_stat_statements",
                "fields": ["calls"],
                "top": 2,
            },
        ],
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let results = structured["results"].as_array().expect("results");
    assert_eq!(results.len(), 4);
    assert_eq!(
        results
            .iter()
            .map(|result| result["ranking"]["fields"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!(["local_blks_hit"]),
            json!(["calls"]),
            json!(["local_blks_hit"]),
            json!(["calls"]),
        ],
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result["ranking"]["top"].clone())
            .collect::<Vec<_>>(),
        vec![json!(1), json!(1), json!(1), json!(2)],
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result["entities"].as_array().expect("entities").len())
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 2],
    );
    assert_eq!(results[0]["entities"][0]["identity"]["query_id"], "2");
    assert_eq!(results[0]["entities"][0]["total"], 40.0);
    assert_eq!(results[1]["entities"][0]["identity"]["query_id"], "1");
    assert_eq!(results[1]["entities"][0]["total"], 10.0);
    assert_eq!(results[2], results[0]);
}

#[test]
fn overview_rejects_a_reversed_window() {
    let config = test_config(std::env::temp_dir());

    let reversed = serde_json::json!({
        "from": 100,
        "to": 0,
        "rankings": [{
            "section": "os_process",
            "fields": ["rmem_kb"],
            "top": 5,
        }],
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::overview::call(&config, reversed, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(message.contains("rankings[0]"), "{message}");
}

#[test]
fn overview_enforces_source_field_count_at_runtime() {
    let config = test_config(std::env::temp_dir());
    for fields in [json!([]), json!(["a", "b", "c", "d", "e"])] {
        let arguments = json!({
            "from": 0,
            "to": 100,
            "rankings": [
                {"section": "os_process", "fields": ["rmem_kb"], "top": 1},
                {"section": "os_process", "fields": fields, "top": 1},
            ],
        })
        .as_object()
        .expect("object")
        .clone();

        let result = crate::mcp::overview::call(&config, arguments, &|| false);

        assert_eq!(result.is_error, Some(true));
        let message = &result.content[0].as_text().expect("text").text;
        assert!(message.contains("rankings[1]"), "{message}");
        assert!(message.contains("1 to 4"), "{message}");
        assert_eq!(
            result.structured_content.expect("structured error")["ranking_index"],
            1,
        );
    }
}

#[test]
fn get_context_reports_the_recorded_range_and_field_catalog() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["recorded_from"], "100");
    assert_eq!(structured["recorded_to"], "301");
    let sections = structured["sections"].as_array().expect("sections array");
    assert!(
        sections.iter().all(|section| !section["logical_name"]
            .as_str()
            .expect("logical name")
            .starts_with("dict.")),
        "store-internal dict.* sections must stay hidden"
    );
    let os_process = sections
        .iter()
        .find(|section| section["logical_name"] == "os_process")
        .expect("os_process section present");
    assert_eq!(
        os_process
            .as_object()
            .expect("section object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["bytes", "fields", "logical_name", "rows", "source_family"]
            .into_iter()
            .collect(),
    );
    assert_no_storage_coordinate_keys(&structured);
    let fields = os_process["fields"].as_array().expect("fields array");
    let rmem = fields
        .iter()
        .find(|field| field["name"] == "rmem_kb")
        .expect("rmem_kb field present");
    assert_eq!(rmem["class"], "gauge");
    assert_eq!(rmem["unit"], "kibibytes");
}

#[test]
fn decimal_time_outputs_reenter_every_shared_time_input() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.append_log_error(200);
    fixture.append_instance_facts();
    fixture.append_postgres_setting("work_mem", "4096", "configuration file");
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let context_result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);
    assert_json_content_matches_structured(&context_result);
    let context = context_result.structured_content.expect("context output");
    assert!(context["recorded_from"].is_string());
    assert!(context["recorded_to"].is_string());
    let overview = crate::mcp::overview::call(
        &config,
        json!({
            "from": context["recorded_from"],
            "to": context["recorded_to"],
            "rankings": [{"section": "os_process", "fields": ["rmem_kb"], "top": 1}],
        })
        .as_object()
        .expect("overview arguments")
        .clone(),
        &|| false,
    );
    assert_json_content_matches_structured(&overview);
    let overview = overview.structured_content.expect("overview output");
    let overview_item = &overview["results"][0];
    assert_eq!(overview_item["coverage"]["state"], "data");
    assert!(overview_item.get("as_of").is_none());
    let events = crate::mcp::events::call(
        &config,
        json!({
            "from": context["recorded_from"],
            "to": context["recorded_to"],
            "representation": "occurrences",
            "limit": 10,
        })
        .as_object()
        .expect("events arguments")
        .clone(),
        &|| false,
    );
    assert_json_content_matches_structured(&events);
    assert_eq!(
        events.structured_content.expect("events output")["occurrences"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let finder = crate::mcp::processes::call(
        &config,
        json!({"at": context["recorded_to"], "limit": 1})
            .as_object()
            .expect("finder arguments")
            .clone(),
        &|| false,
    );
    assert_json_content_matches_structured(&finder);
    let finder = finder.structured_content.expect("finder output");
    assert_eq!(finder["rows"].as_array().map(Vec::len), Some(1));
    assert!(finder.get("as_of").is_none());
    let detail = crate::mcp::row_detail::call(
        &config,
        json!({"detail_ref": finder["rows"][0]["detail_ref"]})
            .as_object()
            .expect("detail arguments")
            .clone(),
        &|| false,
    );
    assert_json_content_matches_structured(&detail);
    let instance = crate::mcp::instance::call(&config, serde_json::Map::new(), &|| false);
    assert_json_content_matches_structured(&instance);
    let replay = crate::mcp::overview::call(
        &config,
        json!({
            "from": context["recorded_from"],
            "to": context["recorded_to"],
            "rankings": [{"section": "os_process", "fields": ["rmem_kb"], "top": 1}],
        })
        .as_object()
        .expect("overview replay arguments")
        .clone(),
        &|| false,
    );
    assert_json_content_matches_structured(&replay);
}

#[test]
fn find_events_accepts_the_temp_files_source() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_temp_files"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));

    let grouped = serde_json::json!({
        "sources": ["pg_log_temp_files"],
        "from": 0,
        "to": 1_000,
        "representation": "groups",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::events::call(&config, grouped, &|| false);
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.expect("structured error")["valid_options"],
        serde_json::json!([
            "pg_log_errors",
            "pg_log_checkpoints",
            "pg_log_autovacuum",
            "pg_log_slow_queries",
            "pg_log_lock_waits",
            "pg_log_lifecycle",
            "pgbouncer_events"
        ])
    );
}

#[test]
fn statements_sort_by_a_derived_field_ranks_by_it() {
    // append_ranked_statements' derived_mean_exec_ms_per_call per queryid:
    // 2 -> 15.0, 1 -> 10.0, 3 -> 5.0 (execution rate over call rate, the
    // shared interval cancels).
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sort": {"field": "derived_mean_exec_ms_per_call", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    let ranked: Vec<f64> = rows
        .iter()
        .map(|row| {
            row["derived_mean_exec_ms_per_call"]
                .as_f64()
                .expect("derived value")
        })
        .collect();
    assert_eq!(ranked, vec![15.0, 10.0, 5.0]);
}

#[test]
fn find_events_reports_truncation_across_segments() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish_and_continue(1_709_164_800_000_000 + 1_000);
    fixture.append_log_error(300);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 2,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["occurrences"]
        .as_array()
        .expect("occurrences array");
    assert_eq!(
        rows.iter()
            .map(|row| {
                let detail =
                    crate::mcp::row_detail::call(&config, detail_arguments(row), &|| false);
                assert_eq!(detail.is_error, Some(false));
                detail.structured_content.expect("event detail")["at"]
                    .as_str()
                    .expect("at")
                    .to_owned()
            })
            .collect::<Vec<_>>(),
        vec!["100", "200"]
    );
    assert_eq!(structured["truncated"], true);
}

#[test]
fn get_instance_returns_host_facts_and_postgresql_settings() {
    let mut fixture = Fixture::new();
    fixture.append_instance_facts();
    fixture.append_postgres_block_size(8_192);
    fixture.append_postgres_setting("work_mem", "4096", "configuration file");
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::instance::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let content: serde_json::Value =
        serde_json::from_str(&result.content[0].as_text().expect("JSON").text)
            .expect("content JSON");
    let structured = result.structured_content.expect("structured content");
    assert_eq!(content, structured);
    let host = structured["host"].as_object().expect("host object");
    // `row_record` renders 64-bit integers as decimal strings.
    assert_eq!(host["clock_ticks_per_sec"], "100");
    assert_eq!(host["page_size_bytes"], "4096");
    assert_eq!(host["hostname"], "fixture-host");
    assert_detail_ref(&structured["host"], "instance_metadata", &[]);
    assert_detail_roundtrip(
        &config,
        &structured["host"],
        "hostname",
        &json!("fixture-host"),
    );
    assert!(structured["host_as_of"].is_string());
    let settings = structured["postgresql_settings"]
        .as_array()
        .expect("settings array");
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0]["name"], "work_mem");
    assert_eq!(settings[0]["setting"], "4096");
    assert_detail_ref(&settings[0], "pg_settings", &[]);
    assert!(structured["settings_as_of"].is_string());
    assert_eq!(structured["settings_scope"], "non_default");
    assert_eq!(structured["settings_returned_count"], "1");
    assert_eq!(structured["settings_defaults_omitted"], true);
    assert_eq!(
        structured["settings_request_all"],
        serde_json::json!({"settings": "all"})
    );
    assert!(structured.get("settings_has_more").is_none());
    assert_no_storage_coordinate_keys(&structured);

    let explicit_non_default = serde_json::json!({"settings": "non_default"})
        .as_object()
        .expect("object")
        .clone();
    let explicit_non_default = crate::mcp::instance::call(&config, explicit_non_default, &|| false)
        .structured_content
        .expect("explicit non-default result");
    assert_eq!(explicit_non_default, structured);

    let all_arguments = serde_json::json!({"settings": "all"})
        .as_object()
        .expect("object")
        .clone();
    let all = crate::mcp::instance::call(&config, all_arguments, &|| false);
    assert_eq!(all.is_error, Some(false));
    let all = all.structured_content.expect("all settings result");
    assert_eq!(all["host"], structured["host"]);
    assert_eq!(all["host_as_of"], structured["host_as_of"]);
    assert_eq!(all["settings_as_of"], structured["settings_as_of"]);
    assert_eq!(all["settings_scope"], "all");
    assert_eq!(all["settings_returned_count"], "2");
    assert_eq!(all["settings_defaults_omitted"], false);
    let all_settings = all["postgresql_settings"]
        .as_array()
        .expect("all settings array");
    assert_eq!(all_settings.len(), 2);
    let block_size = all_settings
        .iter()
        .find(|row| row["name"] == "block_size")
        .expect("default block_size row");
    assert_detail_ref(block_size, "pg_settings", &[]);

    let detail = crate::mcp::row_detail::call(&config, detail_arguments(block_size), &|| false);
    assert_eq!(detail.is_error, Some(false));
    assert_eq!(
        detail.structured_content.expect("structured content")["name"],
        "block_size"
    );
}

#[test]
fn get_instance_reports_unrecorded_parts_as_null_and_empty() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::instance::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert!(structured["host"].is_null());
    assert!(structured["host_as_of"].is_null());
    assert_eq!(
        structured["postgresql_settings"].as_array(),
        Some(&Vec::new())
    );
    assert!(structured["settings_as_of"].is_null());
    assert_eq!(structured["settings_scope"], "non_default");
    assert_eq!(structured["settings_returned_count"], "0");
    assert_eq!(structured["settings_defaults_omitted"], false);
    assert_eq!(
        structured["settings_request_all"],
        serde_json::json!({"settings": "all"})
    );
}

#[test]
fn get_instance_returns_more_than_five_thousand_settings_without_prefix_cutoff() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_settings(5_001, "1", "configuration file");
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = crate::mcp::instance::call(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["settings_returned_count"], "5001");
    assert_eq!(
        structured["postgresql_settings"]
            .as_array()
            .expect("settings")
            .len(),
        5_001
    );
    assert_eq!(structured["settings_defaults_omitted"], false);
}

#[test]
fn overview_reproduced_mixed_unit_request_returns_five_ordered_results() {
    let empty_root = tempfile::tempdir().expect("empty data root");
    let config = test_config(empty_root.path().to_path_buf());
    let arguments = serde_json::json!({
        "from": 0,
        "to": 1_000,
        "rankings": [
            {"section": "pg_log_errors", "fields": ["count"], "top": 1},
            {
                "section": "pg_log_slow_queries",
                "fields": ["total_duration_ms", "count"],
                "top": 2,
            },
            {
                "section": "pg_log_autovacuum",
                "fields": ["pages_removed", "elapsed_ms"],
                "top": 3,
            },
        ],
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let results = structured["results"].as_array().expect("results");
    assert_eq!(results.len(), 5);
    assert_eq!(
        results
            .iter()
            .map(|result| {
                json!({
                    "section": result["ranking"]["section"],
                    "fields": result["ranking"]["fields"],
                    "top": result["ranking"]["top"],
                    "unit": result["unit"],
                })
            })
            .collect::<Vec<_>>(),
        vec![
            json!({"section": "pg_log_errors", "fields": ["count"], "top": 1, "unit": "count"}),
            json!({"section": "pg_log_slow_queries", "fields": ["total_duration_ms"], "top": 2, "unit": "milliseconds"}),
            json!({"section": "pg_log_slow_queries", "fields": ["count"], "top": 2, "unit": "count"}),
            json!({"section": "pg_log_autovacuum", "fields": ["pages_removed"], "top": 3, "unit": "pages"}),
            json!({"section": "pg_log_autovacuum", "fields": ["elapsed_ms"], "top": 3, "unit": "milliseconds"}),
        ],
    );
}

#[test]
fn overview_identity_passes_verbatim_into_the_statements_finder() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let overview_arguments = serde_json::json!({
        "from": 0,
        "to": 1_000,
        "rankings": [{
            "section": "pg_stat_statements",
            "fields": ["total_exec_time"],
            "top": 1,
        }],
    })
    .as_object()
    .expect("object")
    .clone();
    let overview = crate::mcp::overview::call(&config, overview_arguments, &|| false);
    assert_eq!(overview.is_error, Some(false));
    let structured = overview.structured_content.expect("structured content");
    let identity = structured["results"][0]["entities"][0]["identity"]
        .as_object()
        .expect("identity object")
        .clone();
    let query_id = identity["query_id"].clone();
    assert!(query_id.is_string() || query_id.is_number());

    let find_arguments = serde_json::json!({
        "filters": [{"field": "query_id", "op": "eq", "value": query_id}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let found = crate::mcp::postgresql::call_statements(&config, find_arguments, &|| false);
    assert_eq!(found.is_error, Some(false));
    let rows = found.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(rows.len(), 1);
}

#[test]
fn context_and_instance_reject_unexpected_arguments() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let unexpected = serde_json::json!({"unexpected": true})
        .as_object()
        .expect("object")
        .clone();
    for call in [crate::mcp::context::call, crate::mcp::instance::call] {
        assert_eq!(
            call(&config, serde_json::Map::new(), &|| false).is_error,
            Some(false)
        );
        assert_eq!(
            call(&config, unexpected.clone(), &|| false).is_error,
            Some(true)
        );
    }
    let obsolete = serde_json::json!({"additional": true})
        .as_object()
        .expect("object")
        .clone();
    assert_eq!(
        crate::mcp::instance::call(&config, obsolete, &|| false).is_error,
        Some(true)
    );
}

#[tokio::test]
async fn unknown_and_obsolete_tool_names_are_protocol_errors() {
    for name in ["kronika_made_up", "kronika_overview", "kronika_get_context"] {
        let decoded = rpc_response(
            test_config(std::env::temp_dir()),
            json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {"name": name, "arguments": {}}
            }),
            None,
            None,
        )
        .await;
        assert!(
            decoded["error"].is_object(),
            "expected top-level error for {name}: {decoded}"
        );
        assert!(
            decoded["result"].is_null(),
            "expected no result for {name}: {decoded}"
        );
    }
}

#[test]
fn find_events_does_not_truncate_for_matches_past_the_exclusive_window() {
    // Segment two overlaps the window only through another section; its
    // pg_log_errors row sits past `to`, so nothing was omitted.
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish_and_continue(1_709_164_800_000_000 + 1_000);
    fixture.append_pgbouncer_event(300);
    fixture.append_log_error(2_000);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "representation": "occurrences",
        "limit": 2,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["occurrences"]
        .as_array()
        .expect("occurrences array");
    assert_eq!(
        rows.iter()
            .map(|row| {
                let detail =
                    crate::mcp::row_detail::call(&config, detail_arguments(row), &|| false);
                assert_eq!(detail.is_error, Some(false));
                detail.structured_content.expect("event detail")["at"]
                    .as_str()
                    .expect("at")
                    .to_owned()
            })
            .collect::<Vec<_>>(),
        vec!["100", "200"]
    );
    assert_eq!(structured["truncated"], false);
}
