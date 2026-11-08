# Writing recipes and validating them read-only

This guide covers cleanup recipes executable by Core/CLI: file layout, variable mappings, modules, tasks, platform conditions, and validation. See [Configuration reference](CONFIGURATION.md) for complete field definitions and [CLI reference](CLI.md) for commands. Scanning, saving configuration, and executing cleanup are separate actions. Check actual matches before confirming scope.

## 1. Choose the configuration location

Locate the application entry using [default configuration discovery](CONFIGURATION.md#2-default-configuration-discovery), or select it explicitly with `--config`. The examples below use the Linux HOME fallback. Recipe-internal layouts are the same across platforms; use variable mappings and conditional tasks for platform path differences.

## 2. Recommended file layout

This Linux example uses the HOME fallback; every other candidate root has the same internal layout:

```text
~/.config/spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        ├── personal.generated.toml
        └── variables.toml
```

`personal` is the recipe ID; directory and entry file must share that name. Use relative paths for `variable-map` and `include`. Generated content should not depend on local files outside that recipe directory.

`~/.config/spaceprune/config.toml`:

```toml
schema-version = 2
recipe = "personal"

[theme]
id = "aurora"
```

`~/.config/spaceprune/recipes/personal/personal.toml`:

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"

include = [
  "official:minimal",
  "./personal.generated.toml",
]
```

Besides required integer `schema-version = 2`, `variables.toml` contains only mappings from logical paths to platform environments or fixed directories.

Besides required integer `schema-version = 2`, `personal.generated.toml` contains only modules, tasks, path rules, and recommendation policies:

```toml
schema-version = 2

[modules.personal]
name = "Personal data"
```

Every physical configuration file—the entry, `personal.generated.toml`, common includes, and platform-specific includes—is validated independently before merging. It cannot inherit `schema-version` from the entry. The field must be integer `2`, not string `"2"`.

This layout keeps the application entry stable and each cleanup configuration fully contained in its own recipe directory.

## 3. Variable mappings

### 3.1 Basic principle

Task paths use logical variables:

```toml
roots = [{ path = "${TOOL_CACHE}", include = ["*"] }]
```

Do not assume a task variable such as `TOOL_CACHE` directly names an OS environment variable. Put that relationship in `variables.toml`:

```toml
schema-version = 2

[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${DATA_HOME}/tool"

[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"
```

### 3.2 Platform mappings

```toml
[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"

[platforms.macos.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool"

[platforms.windows.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool/Cache"
```

Merge order:

1. Built-in common.
2. Built-in current platform.
3. User common.
4. User current platform.

A same-name user variable replaces the complete definition. A definition cannot provide only `fallback` while omitting `environment` or `value`.

### 3.3 Built-in variables

Available directly:

```text
HOME
CONFIG_HOME
CACHE_HOME
DATA_HOME
STATE_HOME
TEMP_HOME
```

Create new logical variables for tool-specific directories, such as `BROWSER_CACHE`, `AGENT_HOME`, or `PROJECT_ROOT`.

### 3.4 Syntax

Cleanup configuration accepts only:

```text
${NAME}
```

Put fallback values in mapping definitions, not fallback expressions in task paths. A leading `~` uses mapped `HOME`; prefer explicit logical variables for shared configuration.

Include and `variable-map` paths are not expanded. They must be local absolute paths or paths relative to the recipe entry.

## 4. Modules, tasks, tags, and selectors

Module:

```toml
[modules.tool]
name = "Tool data"
description = "Rebuildable data produced by Tool"
order = 20
```

Module IDs cannot use reserved commands `module`, `plugin`, `automation`, `recipe`, `theme`, `language`, `authorization`, `scan`, or `clean`.

Task:

```toml
[modules.tool.tasks.logs]
name = "Logs"
handler = "builtin.filesystem/paths"
tags = ["logs", "tool"]
```

Selector:

```toml
[modules.tool.selectors.logs]
name = "Logs only"
match-tags = ["logs"]
```

The distinctions are:

- `tags` belong to a task and are also passed to its handler.
- `match-tags` defines a selector's query.
- A task must contain every required tag.
- Multiple selectors combine all `match-tags` with AND semantics.
- With no selector, all enabled, platform-matching tasks in the module run.

Selector IDs cannot use fixed long-option names `config`, `scan`, `clean`, `select`, `yes`, `output`, `no-progress`, `help`, or `version`. Ordinary modules may use `all` as a dynamic selector.

The example is invoked with:

```bash
spaceprune tool --logs --scan
```

## 5. Filesystem tasks

```toml
[modules.tool.tasks.logs]
name = "Tool logs"
handler = "builtin.filesystem/paths"
tags = ["tool", "logs"]

[modules.tool.tasks.logs.options]
files = true
directories = true

[[modules.tool.tasks.logs.options.roots]]
path = "${TOOL_HOME}/logs"
include = ["*.log", "archive/**/*.log"]
exclude = ["keep/**", "*.keep"]

[modules.tool.tasks.logs.recommend]
enabled = true
older-than = "30d"
min-size = "1MiB"
```

### 5.1 Root

Each root independently defines:

- `path`: root path.
- `include`: root-relative allowlist.
- `exclude`: root-relative denylist, taking priority.

Prefer explicit allowlists over scanning all of HOME, a volume root, or a parent containing large amounts of unrelated data.

### 5.2 Globs

- `*`: one-level match.
- `?`: one character.
- `[...]`: character class.
- `**`: recursive directories.

Globs are relative to the root. Do not put absolute paths in include or exclude patterns.

### 5.3 Recommendations

- `enabled = false`: everything is optional.
- `older-than`: the latest modification must be older than the threshold; directories use the latest modification anywhere in the tree.
- `min-size`: logical-size threshold.
- When combined, all conditions must hold.

`older-than` is time since last modification, not creation time or retention duration. Do not interpret it as how long the object has existed. Recent or unavailable modification times make items optional.

Uncertain data should remain optional rather than being automatically recommended by permissive rules.

## 6. Cross-platform configuration

If logic is the same but paths differ, prefer variable mappings. If task structure also differs, use conditional includes:

```toml
include = [
  "./common.toml",
  { path = "./linux.toml", platforms = ["linux"] },
  { path = "./windows.toml", platforms = ["windows"] },
]
```

Each of `common.toml`, `linux.toml`, and `windows.toml` must declare integer `schema-version = 2` itself.

For tasks available only on certain platforms:

```toml
[modules.tool.tasks.native-cache]
name = "Native cache"
handler = "builtin.filesystem/paths"
platforms = ["linux", "macos"]
```

Do not duplicate the entire recipe entry to separate platforms unless the recipes themselves are fundamentally different.

## 7. Content not to clean by default

- Documents, photos, source code, and unconfirmed user data.
- Entire browser profiles.
- Password stores, authentication tokens, SSH, GPG, and key directories.
- Configuration files themselves.
- Database primary files.
- Locks, sockets, or active sessions needed by running processes.
- Build artifacts of unclear purpose in working directories.
- Volume roots, all of HOME, all of APPDATA, or all of Application Support.
- System-wide operations requiring root / sudo.

Even when explicitly requested, express cleanup using narrow roots, explicit includes, and conservative recommendation rules.

## 8. Dedicated handlers

For ordinary file paths:

```text
builtin.filesystem/paths
```

Use these only when domain APIs or system commands are needed:

```text
builtin.snapper/snapshots
builtin.system-linux/package-cache
builtin.system-linux/journal
builtin.system-linux/tmpfiles
builtin.system-linux/coredumps
builtin.system-linux/go-modcache  # existing Recipe alias
builtin.developer-tools/go-modcache
builtin.developer-tools/go-buildcache
builtin.developer-tools/cargo-registry-cache
builtin.developer-tools/npm-cache
builtin.developer-tools/docker-build-cache
builtin.developer-tools/docker-dangling-images
builtin.developer-tools/docker-unused-images
builtin.developer-tools/docker-stopped-containers
```

These capabilities are present in the program, but `official:minimal` does not invoke them. Check dedicated-handler fields and options in the configuration reference before use.

A missing developer-tool cache directory is reported as 0 B with no selectable cleanup, rather than an unknown size. Permission failures or a changed directory identity still produce diagnostics; rescanning measures a cache if the tool creates it later.

### Linux system-resource sizes

Journal scans report the logical file footprint of `/var/log/journal` and `/run/log/journal`; crash dumps report their current directory footprint. These are reference sizes. Retention settings and system expiration rules determine what is actually processed, so the entire footprint is not counted as estimated released space.

Temporary-file scanning runs `systemd-tmpfiles --clean --dry-run` against the installed system policy, then reads candidate file metadata. The native preview applies expiration conditions and exclusions without reading file contents or cleaning anything. It requires systemd 256 or newer. Unsupported versions, insufficient access and incomplete previews report why the size is unavailable; the existing cleanup entry point remains usable. Inspection is bounded by time and output limits and responds to cancellation.

Temporary-file rules may cover separately listed crash dumps. Shared or open files may not release space immediately. These logical reference values are not added repeatedly to reclaimable totals. CLI and other Core hosts consume the same measurements.

## 9. External plugins

When configuration requires a non-built-in handler:

1. Confirm the plugin is installed locally.
2. Add a version requirement under `requires.plugins`.
3. Use the complete handler ID.
4. Inspect the plugin first.
5. Validate with a read-only scan.

```toml
[requires.plugins]
"community.example" = "^1.0"
```

The plugin must already be available in a locally installable directory.

## 10. Automation

Generate automation only when explicitly requested by the user:

```toml
[automations.daily]
name = "Daily cleanup"
modules = ["cache", "tool"]
at = "03:00"
```

Before enabling it, verify that:

- All target tasks suit unattended execution.
- Recommendation rules are conservative enough.
- No interactive selection is needed.
- The automation's configuration path is stable.
- A manual `automation run` succeeds.

Generating configuration does not install automation.

## 11. Read-only validation

First:

```bash
spaceprune scan --config /absolute/path/to/config.toml --no-progress
```

For structured inspection:

```bash
spaceprune scan --config /absolute/path/to/config.toml \
  --output json --no-progress
```

Check that:

- `config_source` is the intended `recipes/<ID>/<ID>.toml`.
- Module and task counts match expectations.
- Every location is within the allowed scope.
- There are no missing variables, cycles, or invalid expressions.
- The recipe entry, variable-map, and each include independently declare integer `schema-version = 2`.
- No include is missing and platform names are spelled correctly.
- Recommended items really are rebuildable.
- Excluded items do not appear in results.
- No unexpected system-level directories are included.

Then validate default discovery:

```bash
spaceprune scan --no-progress
```

Check default discovery only after explicit-path validation succeeds.
