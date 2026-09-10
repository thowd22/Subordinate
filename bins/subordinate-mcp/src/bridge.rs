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
//!
//! Alongside the tools it serves [`crate::resources`]: the open project, each
//! sequence and each media item, readable without spending a tool call.
//! Everything a client may cache — both list results and resource reads —
//! carries the `ttlMs` and `cacheScope` hints of the 2026-07-28 specification,
//! and a client that subscribes is told the moment a resource goes stale,
//! straight off the editor's event bus ([`crate::watch`]).
//!
//! Both subscription mechanisms are served, because both are in use: the
//! 2026-07-28 `subscriptions/listen` stream, and `resources/subscribe` for
//! clients that negotiated an earlier version of the protocol.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ResourceUpdatedNotificationParam, ServerCapabilities, ServerInfo, SubscribeRequestParams,
    SubscriptionFilter, UnsubscribeRequestParams,
};
use rmcp::service::{Peer, RequestContext, RoleServer, SubscriptionContext};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::Value;
use sub_core::SubError;
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::backend::Backend;
use crate::resources::{self, Resources, Update};
use crate::tools::{PLUGIN_CALL_METHOD, PLUGIN_TOOLS_METHOD, PluginTools, ToolSet};
use crate::watch::Watch;

/// What an MCP client is told this server is for.
const INSTRUCTIONS: &str = "\
Drives the Subordinate video editor. Every tool is one method of the editor's \
Command API: the same operations the user interface performs, applied to the \
project the editor has open. Mutating tools are undoable through edit_undo and \
edit_redo. Times are exact rationals — an object with a numerator, a \
denominator and a rate — never floating-point seconds. A failed tool answers \
with a JSON error carrying a stable `code`. Read the project rather than \
asking for it: the resources project://current, sequence://{id} and \
media://{id} are the project's own JSON, and a subscriber is told the moment \
an edit makes one of them stale.";

/// The MCP server: a tool set, resources, and a connection to the editor.
#[derive(Debug, Clone)]
pub struct Bridge {
    /// The tools, generated from the committed Command API schema.
    tools: Arc<ToolSet>,
    /// The connection the calls go down.
    backend: Arc<Backend>,
    /// The project, projected into readable resources.
    resources: Resources,
    /// The editor's change events, fanned out to subscribers.
    watch: Arc<Watch>,
    /// What a pre-2026-07-28 client has subscribed to.
    legacy: Arc<Legacy>,
    /// The tools the installed plugins contribute, as the editor last
    /// reported them.
    ///
    /// The compiled-in tool set is the same for every user of a build; this is
    /// not, because it depends on which plugins the editor has installed and
    /// enabled, so it is refreshed on each `tools/list` and consulted when a
    /// call names a tool the schemas do not describe.
    plugins: Arc<Mutex<Arc<PluginTools>>>,
}

/// `resources/subscribe` state: the URIs one session asked to be told about.
///
/// The 2026-07-28 protocol scopes a subscription to its own
/// `subscriptions/listen` request, which also ends it; the older
/// `resources/subscribe` has no such handle, so the set lives with the session
/// and the task that serves it runs until the session does.
#[derive(Debug, Default)]
struct Legacy {
    /// The subscribed URIs.
    uris: Mutex<BTreeSet<String>>,
    /// Whether the task forwarding updates to the client is already running.
    pumping: AtomicBool,
}

impl Bridge {
    /// A bridge offering `tools`, forwarding to `backend`.
    #[must_use]
    pub fn new(tools: Arc<ToolSet>, backend: Arc<Backend>) -> Self {
        Self {
            tools,
            resources: Resources::new(Arc::clone(&backend)),
            watch: Arc::new(Watch::new(Arc::clone(&backend))),
            backend,
            legacy: Arc::new(Legacy::default()),
            plugins: Arc::new(Mutex::new(Arc::new(PluginTools::default()))),
        }
    }

    /// The tools this bridge offers.
    #[must_use]
    pub fn tools(&self) -> &ToolSet {
        &self.tools
    }

    /// The plugin-contributed tools as of the last refresh.
    #[must_use]
    pub fn plugin_tools(&self) -> Arc<PluginTools> {
        Arc::clone(&self.plugins.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Asks the editor which tools its plugins contribute, and remembers the
    /// answer.
    ///
    /// One Command API round trip, so it runs on a blocking task. An editor
    /// that does not serve `plugin.tools` — an older build, or one started
    /// without a plugin host — leaves the previous answer in place instead of
    /// failing the listing: the engine's own tools are what most of a session
    /// uses, and they are compiled in.
    ///
    /// # Errors
    ///
    /// Returns the editor's error when the call fails, and
    /// `mcp.plugin_tools_invalid` when the answer is not a tool listing.
    pub fn refresh_plugin_tools(&self) -> Result<Arc<PluginTools>, SubError> {
        let listing = self.backend.invoke(PLUGIN_TOOLS_METHOD, None)?;
        let plugins = Arc::new(PluginTools::from_listing(&listing)?);
        *self.plugins.lock().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&plugins);
        Ok(plugins)
    }

    /// Every tool this bridge offers right now: the compiled-in ones and the
    /// plugins' own.
    ///
    /// One Command API round trip, so it blocks; `tools/list` runs it on a
    /// blocking task.
    #[must_use]
    pub fn published_tools(&self) -> Vec<rmcp::model::Tool> {
        let plugins = match self.refresh_plugin_tools() {
            Ok(plugins) => plugins,
            Err(error) => {
                debug!(
                    code = error.code.as_str(),
                    "the editor listed no plugin tools"
                );
                self.plugin_tools()
            }
        };
        let mut tools = self.tools.tools().to_vec();
        tools.extend(plugins.tools().iter().cloned());
        tools
    }

    /// The project resources this bridge offers.
    #[must_use]
    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    /// The change feed this bridge's subscriptions read.
    #[must_use]
    pub fn watch(&self) -> &Arc<Watch> {
        &self.watch
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
            return self.call_plugin_tool(request);
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

    /// Runs a call to a plugin-contributed tool.
    ///
    /// The published name carries the plugin id, so the call is one
    /// `plugin.call_tool` round trip naming the plugin, its own tool name and
    /// the client's arguments; the plugin host validates those against the
    /// tool's declared schema before the plugin sees them.
    ///
    /// # Errors
    ///
    /// Returns a protocol error only when no plugin contributes a tool by that
    /// name; the editor's own failures come back as tool-level errors.
    fn call_plugin_tool(&self, request: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        let plugins = self.plugin_tools();
        let Some(route) = plugins.route(&request.name) else {
            return Err(McpError::invalid_params(
                format!("no such tool: {}", request.name),
                None,
            ));
        };
        let arguments = request
            .arguments
            .map_or_else(|| Value::Object(serde_json::Map::new()), Value::Object);
        let params = serde_json::json!({
            "id": route.plugin,
            "tool": route.tool,
            "arguments": arguments,
        });
        debug!(
            tool = %request.name,
            plugin = %route.plugin,
            "forwarding a plugin tool call",
        );
        Ok(
            match self.backend.invoke(PLUGIN_CALL_METHOD, Some(params)) {
                Ok(value) => success(&value),
                Err(error) => {
                    warn!(
                        tool = %request.name,
                        code = error.code.as_str(),
                        "the plugin tool call failed",
                    );
                    failure(&error)
                }
            },
        )
    }

    /// A receiver of every resource update from now on.
    ///
    /// Starting the feed opens a connection and makes a call, so it happens on
    /// a blocking task rather than on the runtime serving the session.
    async fn updates(&self) -> Result<tokio::sync::broadcast::Receiver<Update>, McpError> {
        let watch = Arc::clone(&self.watch);
        tokio::task::spawn_blocking(move || watch.updates())
            .await
            .map_err(|error| {
                McpError::internal_error(format!("the resource watch did not start: {error}"), None)
            })?
            .map_err(|error| protocol_error("the editor's changes cannot be watched", &error))
    }

    /// Starts forwarding updates to a `resources/subscribe` client, once.
    async fn pump_legacy(&self, peer: Peer<RoleServer>) -> Result<(), McpError> {
        if self.legacy.pumping.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let updates = match self.updates().await {
            Ok(updates) => updates,
            Err(error) => {
                // Let the next subscription try again.
                self.legacy.pumping.store(false, Ordering::Release);
                return Err(error);
            }
        };
        let legacy = Arc::clone(&self.legacy);
        tokio::spawn(async move { legacy_updates(peer, updates, &legacy).await });
        Ok(())
    }
}

/// Forwards resource updates to a client that used `resources/subscribe`.
///
/// It ends when the editor's feed does, or when the session is gone and the
/// notifications stop being deliverable.
async fn legacy_updates(
    peer: Peer<RoleServer>,
    mut updates: tokio::sync::broadcast::Receiver<Update>,
    legacy: &Legacy,
) {
    loop {
        let update = match updates.recv().await {
            Ok(update) => update,
            // Falling behind means an unknown number of changes were missed,
            // so everything subscribed is treated as stale.
            Err(RecvError::Lagged(missed)) => {
                warn!(
                    missed,
                    "resource updates were dropped; invalidating everything"
                );
                Update::everything()
            }
            Err(RecvError::Closed) => break,
        };
        let subscribed: Vec<String> = legacy
            .uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|uri| update.touches(uri))
            .cloned()
            .collect();
        for uri in subscribed {
            debug!(%uri, revision = update.revision, "a subscribed resource went stale");
            let notification = ResourceUpdatedNotificationParam::new(uri);
            if peer.notify_resource_updated(notification).await.is_err() {
                return;
            }
        }
        if update.list_changed && peer.notify_resource_list_changed().await.is_err() {
            return;
        }
    }
}

/// A protocol-level error carrying the engine's own structured error.
///
/// A resource read has no place to put a tool-style error result, so the
/// [`SubError`] travels in the JSON-RPC error's `data`, where its stable code
/// is still there to be acted on.
fn protocol_error(message: &'static str, error: &SubError) -> McpError {
    let json = error.to_json();
    if error.code == crate::codes::UNKNOWN_RESOURCE {
        McpError::resource_not_found(message, Some(json))
    } else {
        McpError::internal_error(message, Some(json))
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
        let mut info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_resources_list_changed()
                .build(),
        );
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(INSTRUCTIONS.to_owned());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // The engine's tools come from a schema compiled into this binary, so
        // they are the same for every user of a build and may be cached
        // publicly for an hour. The plugins' tools are not: they are whatever
        // the editor this bridge found has installed and enabled, so they are
        // fetched here, and a listing that carries any of them is this user's
        // alone and goes stale as soon as a plugin is switched on or off.
        let bridge = self.clone();
        let tools = tokio::task::spawn_blocking(move || bridge.published_tools())
            .await
            .map_err(|error| {
                McpError::internal_error(format!("the tool list did not finish: {error}"), None)
            })?;
        let (ttl, scope) = if self.plugin_tools().is_empty() {
            (resources::TOOLS_TTL_MS, CacheScope::Public)
        } else {
            (resources::LIST_TTL_MS, CacheScope::Private)
        };
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(ttl)
            .with_cache_scope(scope))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let resources = self.resources.clone();
        let listed = tokio::task::spawn_blocking(move || resources.list())
            .await
            .map_err(|error| {
                McpError::internal_error(format!("the resource list did not finish: {error}"), None)
            })?
            .map_err(|error| protocol_error("the project could not be read", &error))?;
        Ok(ListResourcesResult::with_all_items(listed)
            .with_ttl_ms(resources::LIST_TTL_MS)
            .with_cache_scope(resources::PROJECT_CACHE_SCOPE))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        // The URI families are a property of this build, not of the project.
        Ok(
            ListResourceTemplatesResult::with_all_items(resources::templates())
                .with_ttl_ms(resources::TOOLS_TTL_MS)
                .with_cache_scope(CacheScope::Public),
        )
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let resources = self.resources.clone();
        let uri = request.uri;
        debug!(%uri, "reading a resource");
        let result = tokio::task::spawn_blocking(move || resources.read(&uri))
            .await
            .map_err(|error| {
                McpError::internal_error(format!("the resource read did not finish: {error}"), None)
            })?
            .map_err(|error| protocol_error("the resource could not be read", &error))?;
        Ok(ReadResourceResponse::Complete(result))
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(resources::accepted_filter(requested))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let accepted = context.accepted().clone();
        let uris = accepted.resource_subscriptions.clone().unwrap_or_default();
        let wants_list = accepted.resources_list_changed == Some(true);
        if uris.is_empty() && !wants_list {
            // Nothing of ours was asked for; hold the stream open until the
            // client ends it, which is what the specification expects.
            context.cancelled().await;
            return Ok(());
        }
        let mut updates = self.updates().await?;
        let sink = context.sink().clone();
        debug!(
            subscribed = uris.len(),
            wants_list, "serving a subscription"
        );
        loop {
            let update = tokio::select! {
                () = context.cancelled() => break,
                received = updates.recv() => match received {
                    Ok(update) => update,
                    Err(RecvError::Lagged(missed)) => {
                        warn!(missed, "resource updates were dropped; invalidating everything");
                        Update::everything()
                    }
                    Err(RecvError::Closed) => break,
                },
            };
            for uri in uris.iter().filter(|uri| update.touches(uri)) {
                debug!(%uri, revision = update.revision, "a subscribed resource went stale");
                if sink.notify_resource_updated(uri.clone()).await.is_err() {
                    return Ok(());
                }
            }
            if wants_list
                && update.list_changed
                && sink.notify_resource_list_changed().await.is_err()
            {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Subscribes one URI, the way clients on protocol versions before
    /// 2026-07-28 do. Those are the clients in the field, so the legacy method
    /// is served alongside [`ServerHandler::listen`].
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        if resources::parse_uri(&request.uri).is_none() {
            return Err(McpError::resource_not_found(
                "no such resource",
                Some(serde_json::json!({ "uri": request.uri })),
            ));
        }
        self.pump_legacy(context.peer.clone()).await?;
        debug!(uri = %request.uri, "subscribed to a resource");
        self.legacy
            .uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.uri);
        Ok(())
    }

    /// The counterpart of the legacy `resources/subscribe` above.
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.legacy
            .uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request.uri);
        Ok(())
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
