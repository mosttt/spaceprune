//! Both request and result use the same authenticated Mach conversation.

use crate::{HelperResponse, OperationContext, PreparedRequest, ResponseOutcome, TaskScanResponse};
use spaceprune_access::{
    PermissionOutcome,
    macos::{
        ServiceKind,
        service::{ConnectFailure, ServiceClient},
    },
};

pub(super) struct Connection(ServiceClient);
impl Connection {
    pub(super) fn start(operation: &OperationContext) -> Result<Self, ConnectFailure> {
        ServiceClient::connect(ServiceKind::Metadata, operation).map(Self)
    }
    pub(super) fn is_alive(&mut self) -> bool {
        self.0.is_alive()
    }
    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        self.0.close()
    }
    pub(super) fn measure(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<ResponseOutcome, PermissionOutcome> {
        let bounded = bounded_request(request, operation);
        self.0.send(&request.descriptor, &bounded)?;
        let response: HelperResponse = self.0.wait(&bounded)?;
        if response.operation != operation.identity() {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        crate::validate_response(response, &request.source_id, &request.nonce)
            .map_err(|_| PermissionOutcome::PolicyBlocked)
    }
    pub(super) fn scan<T: serde::de::DeserializeOwned>(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<(T, bool), PermissionOutcome> {
        let bounded = bounded_request(request, operation);
        self.0.send(&request.descriptor, &bounded)?;
        let response: TaskScanResponse<T> = self.0.wait(&bounded)?;
        if response.schema_version != crate::REQUEST_SCHEMA_VERSION
            || response.operation != operation.identity()
            || response.source_id != request.source_id
            || response.nonce != request.nonce
        {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        response
            .result
            .map(|result| (result, response.cancelled))
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
    }
}

fn bounded_request(request: &PreparedRequest, operation: &OperationContext) -> OperationContext {
    let mut context = operation.child(
        request
            .context
            .deadline
            .saturating_duration_since(std::time::Instant::now()),
    );
    context.budget = context.budget.restricted(request.context.budget);
    context
}
