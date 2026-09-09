//! The MCP server itself: tools in, Command API calls out.
//!
//! The bridge adds no behaviour of its own. `tools/list` is the committed
//! Command API schema ([`ToolSet`]) and `tools/call` is one JSON-RPC round trip
//! to the engine over the local socket, so an agent can do exactly what the
//! user interface can do and no more (docs/PLAN.md §4, §7).
//!
//! A failed call is answered as a tool error rather than a protocol error: the
//! request was well formed and reached the engine, and the [`SubError`] it
//! produced — with its stable code — is what the agent needs to see and act on.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value;
use sub_core::SubError;
use tracing::{debug, warn};

use crate::backend::Backend;
use crate::tools::ToolSet;

/// What an MCP client is told this server is for.
const INSTRUCTIONS: &str = "\
Drives the Subordinate video editor. Every tool is one method of the editor's \
Command API: the same operations the user interface performs, applied to the \
project the editor has open. Mutating tools are undoable through edit_undo and \
edit_redo. Times are exact rationals — an object with a numerator, a \
denominator and a rate — never floating-point seconds. A failed tool answers \
with a JSON error carrying a stable `code`.";

/// The MCP server: a tool set and a connection to the editor.
#[derive(Debug, Clone)]
pub struct Bridge {
    /// The tools, generated from the committed Command API schema.
    tools: Arc<ToolSet>,
    /// The connection the calls go down.
    backend: Arc<Backend>,
}

impl Bridge {
    /// A bridge offering `tools`, forwarding to `backend`.
    #[must_use]
    pub fn new(tools: Arc<ToolSet>, backend: Arc<Backend>) -> Self {
        Self { tools, backend }
    }

    /// The tools this bridge offers.
    #[must_use]
    pub fn tools(&self) -> &ToolSet {
        &self.tools
    }

    /// Runs one tool call, blocking on the Command API round trip.
    ///
    /// # Errors
    ///
    /// Returns an MCP protocol error only when the tool name is not one this
    /// bridge serves; everything the engine itself reports comes back as a
    /// tool-level error result.
    pub fn call(&self, request: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        let Some(method) = self.tools.method(&request.name) else {
            return Err(McpError::invalid_params(
                format!("no such tool: {}", request.name),
                None,
            ));
        };
        let params = request.arguments.map(Value::Object);
        debug!(tool = %request.name, %method, "forwarding a tool call");
        Ok(match self.backend.invoke(method, params) {
            Ok(value) => success(&value),
            Err(error) => {
                warn!(tool = %request.name, code = error.code.as_str(), "the tool call failed");
                failure(&error)
            }
        })
    }
}

/// A result the agent can both read and parse.
fn success(value: &Value) -> CallToolResult {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(value.clone());
    result
}

/// A failure, as the structured JSON error the whole system speaks.
fn failure(error: &SubError) -> CallToolResult {
    let json = error.to_json();
    let text = serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string());
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    result.structured_content = Some(json);
    result
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(INSTRUCTIONS.to_owned());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.tools().to_vec()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let bridge = self.clone();
        // The Command API client is blocking, so the round trip leaves the
        // async runtime rather than holding it up.
        let result = tokio::task::spawn_blocking(move || bridge.call(request))
            .await
            .map_err(|error| {
                McpError::internal_error(format!("the tool call did not finish: {error}"), None)
            })??;
        Ok(CallToolResponse::Complete(result))
    }
}

#[cfg(test)]
mod tests {
    use super::{failure, success};
    use serde_json::json;
    use sub_core::{SubError, codes};

    #[test]
    fn a_result_is_readable_text_and_structured_content() {
        let result = success(&json!({ "revision": 3 }));
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.structured_content, Some(json!({ "revision": 3 })));
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn a_failure_carries_the_stable_error_code() {
        let error = SubError::new(codes::NOT_FOUND, "no such clip").with_detail("clip", "c1");
        let result = failure(&error);
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured content");
        assert_eq!(structured["code"], codes::NOT_FOUND.as_str());
        assert_eq!(structured["details"]["clip"], "c1");
    }
}
