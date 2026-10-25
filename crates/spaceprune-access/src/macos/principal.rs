//! Kernel-derived identity values; this type is never deserialized from a request.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProcessIdentity {
    pub(super) pid: u32,
    pub(super) effective_uid: u32,
    pub(super) real_uid: u32,
    pub(super) audit_session: i32,
}
impl ProcessIdentity {
    pub(super) fn same_login(self, other: Self) -> bool {
        self.effective_uid == other.effective_uid
            && self.real_uid == other.real_uid
            && self.audit_session == other.audit_session
    }
}
