//! Discover installed lifecycle tools without measuring or cleaning their resources.

use super::*;

/// Discover local Linux lifecycle tasks, with recommendation disabled.
///
/// Package-manager selection is resolved now, rather than deferred to another
/// distribution or tool after the caller presents the observed capability.
pub fn discover_system_tasks(context: &OperationContext) -> io::Result<Vec<TaskRequest>> {
    let mut tasks = Vec::new();
    for kind in [
        Kind::PackageCache,
        Kind::Journal,
        Kind::Tmpfiles,
        Kind::Coredumps,
    ] {
        context.check()?;
        let observed = action(kind, &Options::default(), context);
        let available = observed.available;
        if !available {
            continue;
        }
        let options = if matches!(kind, Kind::PackageCache) {
            json!({"manager": observed.program})
        } else {
            json!({})
        };
        tasks.push(TaskRequest {
            module_id: "system-linux".into(),
            task_id: kind.id().into(),
            name: kind.name().into(),
            handler_id: format!("builtin.system-linux/{}", kind.id()),
            options,
            recommend: json!({"enabled": false}),
            tags: vec!["system-lifecycle".into()],
        });
    }
    context.check()?;
    Ok(tasks)
}
