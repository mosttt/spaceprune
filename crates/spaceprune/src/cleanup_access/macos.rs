//! A macOS service consumes one confirmed item per request, then waits for its durable receipt.

use super::*;
use std::io;
mod driver;
#[cfg(target_os = "macos")]
mod native;
mod server;
#[cfg(target_os = "macos")]
pub(super) use native::{Connection, run_if_requested};
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemKey {
    group: usize,
    item: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Prepare { grant: Grant },
    Execute { request: ExecuteRequest },
    Acknowledge { item: ItemKey, accepted: bool },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case", deny_unknown_fields)]
enum NativeReply {
    Ready,
    Item { result: HandlerCleanResult },
    Acknowledged,
}

trait Backend: Send {
    fn request(
        &mut self,
        request: Request,
        context: &OperationContext,
        events: &mut dyn FnMut(Reply) -> PluginResult<()>,
    ) -> PluginResult<NativeReply>;
}
