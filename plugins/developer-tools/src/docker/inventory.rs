//! Object enumeration and exact, non-forced lifecycle commands.
use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) struct Object {
    pub id: String,
    pub name: String,
    pub bytes: Option<u64>,
}
pub(super) fn valid_id(kind: DockerKind, id: &str) -> bool {
    match kind {
        DockerKind::BuildCache => {
            !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric())
        }
        DockerKind::DanglingImages | DockerKind::UnusedImages => {
            id.strip_prefix("sha256:").is_some_and(hex_id)
        }
        DockerKind::StoppedContainers => hex_id(id),
    }
}
fn hex_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn rows(value: &str) -> PluginResult<Vec<Value>> {
    value
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|_| invalid()))
        .collect()
}

pub(super) fn list(
    kind: DockerKind,
    binding: &Binding,
    runner: &dyn Runner,
    context: &OperationContext,
) -> PluginResult<Vec<Object>> {
    let run = |args: &[&str]| runner.run(Some(&binding.endpoint), args, context);
    let mut objects = Vec::new();
    match kind {
        DockerKind::BuildCache => {
            // "default" is Buildx's reserved Docker-context builder, never the user's selected
            // docker-container/kubernetes/remote builder. --host fixes its Engine transport.
            for row in rows(&run(&[
                "buildx",
                "du",
                "--builder",
                "default",
                "--format",
                "json",
            ])?)? {
                if row.get("Reclaimable").and_then(Value::as_bool) != Some(true) {
                    continue;
                }
                let id = row
                    .get("ID")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?
                    .to_owned();
                let bytes = row
                    .get("Size")
                    .and_then(Value::as_str)
                    .and_then(exact_bytes);
                let description = row
                    .get("Description")
                    .and_then(Value::as_str)
                    .unwrap_or(&id);
                objects.push(Object {
                    id: id.clone(),
                    name: format!("{} · {id}", description),
                    bytes,
                });
            }
        }
        DockerKind::DanglingImages | DockerKind::UnusedImages => {
            let filter = if kind == DockerKind::DanglingImages {
                "dangling=true"
            } else {
                "dangling=false"
            };
            let ids = run(&[
                "image",
                "ls",
                "--all",
                "--quiet",
                "--no-trunc",
                "--filter",
                filter,
            ])?;
            for id in ids
                .lines()
                .filter(|s| !s.is_empty())
                .collect::<BTreeSet<_>>()
            {
                if !valid_id(kind, id) {
                    return Err(invalid());
                }
                // Include stopped-container references too; running-state checks alone are insufficient.
                if !run(&[
                    "container",
                    "ls",
                    "--all",
                    "--quiet",
                    "--filter",
                    &format!("ancestor={id}"),
                ])?
                .trim()
                .is_empty()
                {
                    continue;
                }
                let details = run(&[
                    "image",
                    "inspect",
                    "--format",
                    r#"{"ID":{{json .Id}},"Size":{{json .Size}},"Tags":{{json .RepoTags}}}"#,
                    id,
                ])?;
                let rows = rows(&details)?;
                let row = rows
                    .first()
                    .filter(|_| rows.len() == 1)
                    .ok_or_else(invalid)?;
                if row["ID"].as_str() != Some(id) {
                    return Err(invalid());
                }
                let tags = row["Tags"]
                    .as_array()
                    .map(|tags| {
                        tags.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                if kind == DockerKind::UnusedImages && tags.is_empty() {
                    continue;
                }
                objects.push(Object {
                    id: id.into(),
                    name: if tags.is_empty() { id.into() } else { tags },
                    bytes: row["Size"].as_u64(),
                });
            }
        }
        DockerKind::StoppedContainers => {
            let ids = run(&[
                "container",
                "ls",
                "--all",
                "--quiet",
                "--no-trunc",
                "--filter",
                "status=exited",
                "--filter",
                "status=created",
                "--filter",
                "status=dead",
            ])?;
            for id in ids
                .lines()
                .filter(|s| !s.is_empty())
                .collect::<BTreeSet<_>>()
            {
                if !valid_id(kind, id) {
                    return Err(invalid());
                }
                let details = run(&[
                    "container",
                    "inspect",
                    "--size",
                    "--format",
                    r#"{"ID":{{json .Id}},"Size":{{json .SizeRw}},"Name":{{json .Name}},"State":{{json .State.Status}}}"#,
                    id,
                ])?;
                let rows = rows(&details)?;
                let row = rows
                    .first()
                    .filter(|_| rows.len() == 1)
                    .ok_or_else(invalid)?;
                if row["ID"].as_str() != Some(id) {
                    return Err(invalid());
                }
                if !matches!(row["State"].as_str(), Some("exited" | "created" | "dead")) {
                    continue;
                }
                objects.push(Object {
                    id: id.into(),
                    name: row["Name"]
                        .as_str()
                        .unwrap_or(id)
                        .trim_start_matches('/')
                        .into(),
                    bytes: row["Size"].as_u64(),
                });
            }
        }
    }
    if objects.iter().any(|object| !valid_id(kind, &object.id)) {
        return Err(invalid());
    }
    Ok(objects)
}

// Docker's human-readable kB/MB/GB fields are rounded, not exact byte measurements.
// Keep them unknown rather than turning a rounded display into false byte precision.
fn exact_bytes(size: &str) -> Option<u64> {
    size.strip_suffix('B')?.parse().ok()
}

pub(super) fn remove_args(kind: DockerKind, id: &str) -> Vec<String> {
    let args: Vec<&str> = match kind {
        DockerKind::BuildCache => vec![
            "buildx",
            "prune",
            "--builder",
            "default",
            "--force",
            "--all",
            "--filter",
        ],
        DockerKind::DanglingImages | DockerKind::UnusedImages => vec!["image", "rm", "--no-prune"],
        DockerKind::StoppedContainers => vec!["container", "rm"],
    };
    let mut args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
    // Buildx's id filter is a regular expression, so anchor the validated opaque ID.
    args.push(if kind == DockerKind::BuildCache {
        format!("id=^{id}$")
    } else {
        id.to_owned()
    });
    args
}

// Existence is deliberately independent of eligibility: a newly busy cache/container
// must not disappear from this verification merely because it became protected.
pub(super) fn exists(
    kind: DockerKind,
    binding: &Binding,
    id: &str,
    runner: &dyn Runner,
    context: &OperationContext,
) -> PluginResult<bool> {
    let args = match kind {
        DockerKind::BuildCache => vec!["buildx", "du", "--builder", "default", "--format", "json"],
        DockerKind::DanglingImages | DockerKind::UnusedImages => {
            vec!["image", "ls", "--all", "--quiet", "--no-trunc"]
        }
        DockerKind::StoppedContainers => vec!["container", "ls", "--all", "--quiet", "--no-trunc"],
    };
    let output = runner.run(Some(&binding.endpoint), &args, context)?;
    if kind == DockerKind::BuildCache {
        let mut found = false;
        for row in rows(&output)? {
            let current = row["ID"].as_str().ok_or_else(invalid)?;
            if !valid_id(kind, current) {
                return Err(invalid());
            }
            found |= current == id;
        }
        Ok(found)
    } else {
        let ids = output
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if ids.iter().any(|current| !valid_id(kind, current)) {
            return Err(invalid());
        }
        Ok(ids.contains(&id))
    }
}
