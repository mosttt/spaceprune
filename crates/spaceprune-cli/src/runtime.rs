//! Core session setup and process cancellation.

use anyhow::{Context, Result};
use spaceprune::api::{CancellationToken, OperationContext, ResolvedConfig, SpacePrune};

pub(crate) use crate::progress::TextProgress;

/// 从已解析配置创建应用核心会话。
pub(crate) fn build_engine(
    config: ResolvedConfig,
    context: &OperationContext,
) -> Result<SpacePrune> {
    SpacePrune::from_config_in_context(None, config, context)
}

/// Start a separate operation while preserving process-wide cancellation and terminal policy.
pub(crate) fn operation_context(cancellation: &CancellationToken) -> OperationContext {
    OperationContext::workflow(
        cancellation.new_operation(),
        spaceprune::api::linux_authorization_interaction(),
    )
}

/// 创建由进程 Ctrl+C 信号驱动的取消令牌。
pub(crate) fn signal_cancellation() -> Result<CancellationToken> {
    let cancellation = CancellationToken::default();
    let signal = cancellation.clone();
    ctrlc::set_handler(move || signal.cancel())
        .context(crate::i18n::text("terminal.ctrlc_failed"))?;
    Ok(cancellation)
}
