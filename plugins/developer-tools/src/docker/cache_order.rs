//! A reclaimable parent is removable only after its selected child references are released.
use super::*;
use std::collections::BTreeSet;

pub(super) fn selected<'a>(
    kind: DockerKind,
    items: &'a [CleanupItem],
    runner: &dyn Runner,
    context: &OperationContext,
) -> Vec<&'a CleanupItem> {
    let original = || items.iter().collect();
    if kind != DockerKind::BuildCache || items.len() < 2 || context.is_stopped() {
        return original();
    }
    let Ok(binding) = cli::probe(runner, context) else {
        return original();
    };
    // Ordering conveys no authorization: every item is still validated and rechecked at execution.
    if items.iter().any(|item| {
        !serde_json::from_value::<Selection>(item.opaque.clone())
            .is_ok_and(|selection| selection.binding == binding && selection.kind == kind)
    }) {
        return original();
    }
    let Ok(objects) = inventory::list(kind, &binding, runner, context) else {
        return original();
    };
    order(items, &objects)
}

fn order<'a>(items: &'a [CleanupItem], objects: &[inventory::Object]) -> Vec<&'a CleanupItem> {
    let mut remaining: Vec<_> = items.iter().collect();
    let mut ordered = Vec::with_capacity(items.len());
    while !remaining.is_empty() {
        let ids: BTreeSet<_> = remaining.iter().map(|item| item.id.as_str()).collect();
        let referenced: BTreeSet<_> = objects
            .iter()
            .filter(|object| ids.contains(object.id.as_str()))
            .flat_map(|object| object.parents.iter().map(String::as_str))
            .collect();
        let count = remaining.len();
        remaining.retain(|item| {
            if referenced.contains(item.id.as_str()) {
                true
            } else {
                ordered.push(*item);
                false
            }
        });
        if remaining.len() == count {
            // Malformed/cyclic metadata must not loop or broaden the exact selected IDs.
            ordered.extend(remaining);
            break;
        }
    }
    ordered
}
