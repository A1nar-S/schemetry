//! Model Context Protocol server, served over stdio by the `schemetry-mcp` binary.
//!
//! stdout carries the JSON-RPC stream, so nothing in this process may print to it;
//! diagnostics go to stderr. All policy, audit and database work lives in
//! `schemetry_core::services::mcp_gateway`; this crate only speaks the protocol.

use std::ffi::OsString;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemetry_core::services::mcp_gateway::{
    ApplyDdlArgs, CompareObjectArgs, CompareSchemasArgs, GenerateFixScriptArgs, GetObjectDdlArgs,
    GetObjectErrorsArgs, ListInvalidObjectsArgs, ListSchemaObjectsArgs, McpGateway, QueryTableArgs,
    RunQueryArgs, SaveMigrationScriptArgs, ToolError,
};
use serde_json::Value;

pub const SERVER_NAME: &str = "schemetry";

pub const USAGE: &str = "\
Schemetry MCP server (stdio transport)

USAGE:
    schemetry-mcp [--db <PATH>]

OPTIONS:
    --db <PATH>    SQLite database to use (default: $SCHEMETRY_DB, then the OS app-data dir)
    -h, --help     Print this help
    -V, --version  Print the version";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve { db_override: Option<OsString> },
    Help,
    Version,
}

pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mut db_override: Option<OsString> = None;

    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("-V" | "--version") => return Ok(Command::Version),
            Some("--db") => {
                let value = args
                    .next()
                    .filter(|value| !value.is_empty())
                    .ok_or("--db requires a database path")?;
                set_db(&mut db_override, value)?;
            }
            Some(other) if other.starts_with("--db=") => {
                let value = OsString::from(&other["--db=".len()..]);
                if value.is_empty() {
                    return Err("--db requires a database path".to_string());
                }
                set_db(&mut db_override, value)?;
            }
            _ => {
                return Err(format!(
                    "Unrecognized argument: {}",
                    argument.to_string_lossy()
                ));
            }
        }
    }

    Ok(Command::Serve { db_override })
}

fn set_db(slot: &mut Option<OsString>, value: OsString) -> Result<(), String> {
    if slot.is_some() {
        return Err("--db may only be specified once".to_string());
    }
    *slot = Some(value);
    Ok(())
}

#[derive(Clone)]
pub struct SchemetryMcp {
    gateway: Arc<McpGateway>,
}

fn client_name(context: &RequestContext<RoleServer>) -> String {
    context
        .client_info()
        .map(|info| format!("{} {}", info.name, info.version))
        .unwrap_or_else(|| "unknown".to_string())
}

fn render(result: Result<Value, ToolError>) -> Result<CallToolResult, McpError> {
    Ok(match result {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    })
}

/// Runs a gateway call off the async threads — it blocks on databases and the keychain.
async fn run_blocking<F>(call: F) -> Result<CallToolResult, McpError>
where
    F: FnOnce() -> Result<Value, ToolError> + Send + 'static,
{
    match tokio::task::spawn_blocking(call).await {
        Ok(result) => render(result),
        Err(error) => Err(McpError::internal_error(error.to_string(), None)),
    }
}

#[tool_router]
impl SchemetryMcp {
    pub fn new(gateway: Arc<McpGateway>) -> Self {
        Self { gateway }
    }

    #[tool(
        description = "List the database connections this agent may use, with each one's id, engine, schema and the permissions its policy grants. Start here: every other tool takes a connection id from this list."
    )]
    async fn list_connections(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.list_connections(&client)).await
    }

    #[tool(
        description = "List the objects (tables, views, packages, …) of a connection's schema, optionally filtered by object type. Hidden objects are omitted."
    )]
    async fn list_schema_objects(
        &self,
        Parameters(args): Parameters<ListSchemaObjectsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.list_schema_objects(&client, args)).await
    }

    #[tool(
        description = "Read the DDL of one schema object (no table data). For packages the type matters: PACKAGE returns the specification only, PACKAGE BODY the body only, and on Oracle PACKAGE SPEC AND BODY returns both as one script (spec, a `/` line, body, a `/` line). The response's `includes` lists which parts are present."
    )]
    async fn get_object_ddl(
        &self,
        Parameters(args): Parameters<GetObjectDdlArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.get_object_ddl(&client, args)).await
    }

    #[tool(
        description = "List the objects of a connection's schema that the database flags as invalid (Oracle: STATUS = INVALID, e.g. packages, views, triggers that fail to compile), optionally filtered by object type. Use get_object_errors for the compiler messages. PostgreSQL has no such concept and returns a 'not applicable' result. Hidden objects are omitted."
    )]
    async fn list_invalid_objects(
        &self,
        Parameters(args): Parameters<ListInvalidObjectsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.list_invalid_objects(&client, args)).await
    }

    #[tool(
        description = "Read the compile errors and warnings (line, position, severity, message) of one stored object, e.g. after fixing code, to verify it now compiles. Oracle only; PostgreSQL returns a 'not applicable' result."
    )]
    async fn get_object_errors(
        &self,
        Parameters(args): Parameters<GetObjectErrorsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.get_object_errors(&client, args)).await
    }

    #[tool(
        description = "Save DDL as code and/or migration files in the schema's configured folder, using the app's naming and folder rules. Give exactly one of `ddl` (inline, max 1 MB) or `source`, which keeps the content out of the conversation: {\"server\": true} fetches the object's DDL from the connection itself (needs DDL read permission); {\"file\": \"relative/path.sql\"} copies a file from the schema's folder byte-for-byte (max 8 MB; absolute paths, `..` and symlinks leaving the folder are refused). Use dry_run to plan first: it reports each file's path, size, sha256, line count, encoding, line endings and, for existing files, an added/removed line diff, without echoing content (set include_content for small files). To keep an Oracle package's spec and body in one .pck file, use object_type PACKAGE SPEC AND BODY with ddl (or source) holding CREATE PACKAGE, a `/` line, then CREATE PACKAGE BODY; PACKAGE and PACKAGE BODY save each part on its own."
    )]
    async fn save_migration_script(
        &self,
        Parameters(args): Parameters<SaveMigrationScriptArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.save_migration_script(&client, args)).await
    }

    #[tool(
        description = "Compare the schema of a reference connection with one or more others (same database engine) and list the discrepancies."
    )]
    async fn compare_schemas(
        &self,
        Parameters(args): Parameters<CompareSchemasArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.compare_schemas(&client, args)).await
    }

    #[tool(
        description = "Compare schemas like compare_schemas and generate an idempotent SQL script that brings the targets in line with the reference. The script is returned, never executed."
    )]
    async fn generate_fix_script(
        &self,
        Parameters(args): Parameters<GenerateFixScriptArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.generate_fix_script(&client, args)).await
    }

    #[tool(
        description = "Compare a local DDL file in the schema's folder (path relative to it, e.g. code/app/ORDERS.sql) with one object's DDL on the server, entirely inside Schemetry: the file content is not sent through you. Returns {\"identical\": true} or a bounded unified diff ('-' server, '+' file) with counts. Line endings, trailing whitespace and, for Oracle, the CREATE header (OR REPLACE, EDITIONABLE, schema prefix, case) are normalised first."
    )]
    async fn compare_object(
        &self,
        Parameters(args): Parameters<CompareObjectArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.compare_object(&client, args)).await
    }

    #[tool(
        description = "Read rows from one allowed table using structured columns, filters and ordering (no SQL text). Limited to the policy's table/column allowlist, row cap and timeout."
    )]
    async fn query_table(
        &self,
        Parameters(args): Parameters<QueryTableArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.query_table(&client, args)).await
    }

    #[tool(
        description = "Run one read-only SELECT. Only available when the connection's policy allows raw SQL; the statement is checked against the table allowlist and runs in a read-only transaction with a row cap and timeout."
    )]
    async fn run_query(
        &self,
        Parameters(args): Parameters<RunQueryArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.run_query(&client, args)).await
    }

    #[tool(
        description = "Deploy one CREATE [OR REPLACE] statement (view, procedure, function, package spec+body, trigger, type) to a dev/test connection's own schema, then check compile errors. Only for connections whose policy allows it; the statement is validated (nothing but that one CREATE) and the user normally has to approve it in Schemetry first: the first call returns status 'pending' with an approval id — ask the user to approve, then call again with identical arguments. Give ddl text or a file path relative to the schema folder."
    )]
    async fn apply_ddl(
        &self,
        Parameters(args): Parameters<ApplyDdlArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let (gateway, client) = (Arc::clone(&self.gateway), client_name(&context));
        run_blocking(move || gateway.apply_ddl(&client, args)).await
    }
}

#[tool_handler]
impl ServerHandler for SchemetryMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Schemetry gives controlled access to the Oracle/PostgreSQL connections configured \
                 in the Schemetry app. Call list_connections first; what each connection allows is \
                 decided by its policy, and every call is recorded in an audit log.",
            )
    }
}

/// Initialise the shared SQLite/keychain bootstrap and serve MCP over stdio until the
/// client disconnects.
pub fn run_stdio(db_override: Option<OsString>) -> Result<()> {
    let services = Arc::new(schemetry_core::init(db_override)?);
    let gateway = Arc::new(McpGateway::new(&services));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to start async runtime")?;

    // `services` stays alive outside the async block: it owns its own tokio runtimes,
    // and dropping those from inside this runtime's context panics.
    let outcome = runtime.block_on(async {
        let service = SchemetryMcp::new(gateway)
            .serve(stdio())
            .await
            .context("Failed to start MCP server")?;
        service.waiting().await.context("MCP server error")?;
        Ok(())
    });
    drop(runtime);
    drop(services);
    outcome
}

#[cfg(test)]
#[path = "tests/args.rs"]
mod tests;
