//! Find MCP tools that advertise a sidebar entrypoint and read their Markdown resource.

use super::StatuslineProvider;
use super::inventory;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::McpResourceContent;
use codex_app_server_protocol::McpResourceReadParams;
use codex_app_server_protocol::McpResourceReadResponse;
use codex_app_server_protocol::RequestId;
use codex_protocol::ThreadId;
use color_eyre::Result;
use color_eyre::eyre::eyre;
use futures::TryStreamExt;
use uuid::Uuid;

pub(crate) const ENTRYPOINT: &str = "sidebar";
const STATE_META: &str = "com.openai/codex-sidebar-state";

pub(crate) async fn discover(client: &AppServerRequestHandle) -> Result<Vec<StatuslineProvider>> {
    let pages = inventory::global_pages(client);
    tokio::pin!(pages);
    let mut found = Vec::new();
    while let Some(servers) = pages.try_next().await? {
        for server in servers {
            let Ok(tools) = &server.tools else {
                continue;
            };
            found.extend(tools.values().filter_map(|tool| {
                StatuslineProvider::from_tool_entrypoint(
                    &server.name,
                    server.plugin_id.as_ref(),
                    tool,
                    ENTRYPOINT,
                )
            }));
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

/// The server learns which thread is on screen from `_meta`; a plain MCP read carries no thread.
pub(crate) async fn read(
    client: &AppServerRequestHandle,
    thread: ThreadId,
    provider: &StatuslineProvider,
) -> Result<String> {
    let response: McpResourceReadResponse = client
        .request_typed(ClientRequest::McpResourceRead {
            request_id: RequestId::String(Uuid::new_v4().to_string()),
            params: McpResourceReadParams {
                thread_id: Some(thread.to_string()),
                server: provider.id.server.as_str().to_owned(),
                environment_id: None,
                uri: provider.uri.clone(),
                connector_id: None,
                origin_call_id: None,
                target: None,
                meta: Some(serde_json::Map::from_iter([(
                    STATE_META.into(),
                    serde_json::json!({ "thread": { "id": thread.to_string() } }),
                )])),
            },
        })
        .await?;
    match response.contents.as_slice() {
        [McpResourceContent::Text { uri, text, .. }] if *uri == provider.uri => Ok(text.clone()),
        _ => Err(eyre!(
            "expected exactly one text resource at {}",
            provider.uri
        )),
    }
}
