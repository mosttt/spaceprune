# CLI reference

This page covers the complete command interface, terminal controls, and common configuration examples. New users should start with the [quick start](../../../README.md#quick-start). See [Configuration reference](CONFIGURATION.md) for all fields, complete personal configuration, and common errors.

## Fixed command interface

Without a mode, CLI opens interactive selection. `scan` is read-only; `clean` cleans recommended items after confirmation. Only an explicit `--yes` skips that confirmation.

```text
spaceprune [module] [module selectors] [--scan|--clean|--select] [options]
spaceprune scan [--module <id>] [--selector <id>...] [--task <id>...] [options]
spaceprune clean [--module <id>] [--selector <id>...] [--task <id>...] [options]
spaceprune authorization status
spaceprune authorization install [--graphical]
spaceprune module list [--all]
spaceprune plugin <catalog|list|inspect|install|remove|enable|disable>
spaceprune automation <list|install|uninstall|run>
spaceprune recipe
spaceprune recipe list
spaceprune recipe set <recipe-id>
spaceprune theme
spaceprune theme list
spaceprune theme set <theme-id>
spaceprune language [list|set <system|en|zh-CN>]
```

`spaceprune module list` lists enabled modules in configuration order; `--all` also includes disabled modules. `module`, `plugin`, `automation`, `recipe`, `theme`, `language`, `authorization`, `scan`, and `clean` are reserved commands and cannot be module IDs. Within other modules, `--all` remains a dynamic selector. Fixed option names `config`, `scan`, `clean`, `select`, `yes`, `output`, `no-progress`, `help`, and `version` cannot be selector IDs.

`scan` / `clean` are stable scripting entry points independent of recipe module names. `--module` selects exactly one module; repeated `--selector` filters tasks by tags with AND semantics; repeated `--task` restricts exact tasks. Both `--selector` and `--task` require `--module`. Dynamic forms such as `spaceprune cache --scan` remain available for discovering and invoking configured modules. Fixed subcommands cannot be combined with legacy `--scan`, `--clean`, or `--select` mode flags.

Main options:

| Option | Purpose |
| --- | --- |
| `--config <PATH>` | Use the specified application `config.toml` |
| `--scan` | Scan only |
| `--clean` | Clean recommended items |
| `--select` | Select interactively |
| `--yes`, `-y` | Skip cleanup confirmation; only for `clean` / `--clean` |
| `--output <human\|json>` | Select output format |
| `--no-progress` | Disable progress on the diagnostic stream |

Subcommand help shows only relevant options. Recipe, theme, language, and plugin management do not list cleanup modes or `-y`; `automation run` retains `--no-progress`, while read-only scanning has no `-y`. System authorization and cleanup confirmation are separate: `-y` does not bypass system authorization.

Ctrl+C during scanning or cleanup cooperatively cancels remaining work and returns exit code 130; the JSON report also sets `cancelled` to true. The terminal temporarily switches to a separate progress screen showing the current operation, elapsed time, returned check/cleanup counts, and real counts supplied by the current step. Elapsed time and animation continue when a tool has no new output, indicating that work is ongoing. No guessed percentage is shown without a reliable total. Snapper logical-size measurement and Btrfs synchronization during `delete --sync` are identified separately. After stopping, an unfinished system command is shown as awaiting completion. Completed deletions are not undone, and stopping does not retry a deletion command.

Before system password entry, CLI returns to the original terminal, pauses animation, and retains the authorization explanation. It restores progress afterward. Valid sudo authentication is reused; refreshing progress does not request another password. Scanning returns to the original terminal with an elapsed-time summary before selection/confirmation. Cleanup restores command history and displays final results. Resizing redraws only the separate progress screen; short windows show fewer lines, preserving earlier commands and authorization records. Progress neither reads keyboard input nor changes echo. It writes only to stderr, leaving stdout JSON parseable. Redirected stderr or `TERM=dumb` uses plain logs; `--no-progress` disables all progress and phase summaries.

In interactive selection, `h/l` collapses/expands the current module/task, and `-`/`+` collapses/expands all modules. The terminal title temporarily becomes `SpacePrune — Interactive cleanup`, then returns to its original title on exit.

The `Logical size` column reports logical file size. Snapper snapshots are measured by reading file and directory metadata, not file contents. This may take time; progress identifies logical-size measurement. Insufficient permission inside a snapshot requests authorization according to the recipe's `elevation`. Listing snapshots without a permission problem does not request a password, and valid sudo authentication can be reused. Refused authorization or incomplete measurement leaves the size `unknown`, while snapshot details remain visible.

Complete group sizes show a normal value. Partially measured groups show a known subtotal with `+`; fully unmeasured groups show `unknown`, never a false `0 B`. Snapper `#0` represents the current system, not a cleanable historical snapshot. The plugin names it “Current system (#0)” and marks logical size not applicable. CLI shows `N/A` (“不适用” in Chinese), excluding it from unknown counts. Tasks and modules containing only the current system also show N/A. Failed measurement of real historical snapshots remains unknown; ordinary empty directories still show `0 B`.

The top selection summary and confirmation separate ordinary processed-size estimates from selected snapshot groups. Snapshots do not inflate ordinary unknown-item counts. Classification follows plugin accounting metadata, including external plugins that declare snapshot-group accounting. Snapshots may share blocks, so their logical size in the table is not an estimate of space freed by cleanup; item details explain the distinction.

When selected ordinary items have unknown processed size, press `U` to locate the next one. This expands its ancestors and clears a search filter if needed, without changing selection. Details show its original location and processed-size estimate; project and module rows retain their logical-size display.

After cleanup, ordinary files show processed size and unknown-item count. Snapper snapshots are excluded from that unknown count and reported separately as successfully processed snapshot groups (a pre/post pair counts as one). Each actual deletion batch also shows the volume's net free-space change, such as `root: +2.00 GiB`: the difference between measurements before deletion and after `snapper delete --sync` returns. Other activity on the volume can affect it. A negative value means less free space; `+0 B` means no net change. Measurement failures show `unavailable` with a reason, without changing deletion results or adding snapshots to ordinary-file unknown counts.

Snapshots already missing during pre-execution revalidation show `already absent`, not newly deleted. Their additional space freed in this run is `0 B`, not `unavailable` due to a missing deletion measurement. If the batch also performs real deletions, its measured volume change is retained.

`--output json` retains original per-item results and adds `cleanup_size_summary`. Already-absent snapshot groups appear in `snapshot_already_absent_group_count`; batch measurements appear in `snapshot_space_changes`. A single combined deletion is recorded once, not once per task.

When ordinary Linux file cleanup lacks permission, affected confirmed items are collected and system authorization is requested according to the recipe. Valid sudo authentication is reused; only expiry requires a password. Refusal does not prompt again per file. Valid sudo authentication obtained for Snapper can also be reused, but listing or read-only access does not grant deletion permission. Set `elevation = "never"` in filesystem task options to forbid automatic elevation.

Permission failures show a small set of examples and a total count; JSON retains all failures. Rescan items whose cleanup was not confirmed. Authorization failure does not mean the entire batch was unchanged.

## Linux system authorization components

Ordinary file and Snapper scans begin with current permissions, requesting system read-only authorization only when necessary; live connections can be reused. A foreground operation needing permission verifies and installs/updates bundled components on demand, then resumes. Preparation or authorization failure shows the specific reason. These commands inspect status or perform manual maintenance; they are not prerequisites for an ordinary scan:

```bash
spaceprune authorization status
spaceprune authorization install
```

`status` requests no permission, supports `--output json`, and returns 0 when ready or 1 otherwise. Status and installation do not depend on the current recipe/configuration loading successfully. `install` obtains same-version read-only, cleanup, and installer components from beside CLI and uses the Core installer to install the set into `/usr/libexec/spaceprune/native-6-4`, then checks root ownership, path permissions, digests, and each protocol. Terminal mode uses foreground sudo with hidden password entry; Ctrl+C cancels. In a desktop session, `install --graphical` opens system authentication. Noninteractive runs try only existing system authentication. Ordinary readable tasks and noninteractive tasks do not install automatically.

Read and cleanup permissions remain purpose-specific. Source users first build CLI and its three adjacent components:

```bash
cargo build --locked -p spaceprune-cli -p spaceprune-readonly-helper
./target/debug/spaceprune authorization install
```

## Shared display language

CLI and Desktop share the top-level `language` field in application `config.toml`. The default, `system`, follows the OS. Supported languages are English (`en`) and Simplified Chinese (`zh-CN`); unsupported or unrecognized system languages fall back to English. Manual settings override the OS language.

```bash
spaceprune language
spaceprune language list
spaceprune language set zh-CN
spaceprune language set en
spaceprune language set system
```

`language` displays the preference and resolved language read-only. `list` lists `en` (English), `zh-CN` (简体中文), and the separate `system` follow-system mode. `set` saves the shared preference. Use `--config PATH` for a specific application entry. Reading the setting does not depend on a usable active recipe; language is not stored in recipes. Machine JSON preserves stable keys, IDs, and existing English product messages. Paths, user names, third-party plugin text, and raw system output remain unchanged.

## Terminal themes

CLI provides four styles, with Aurora as default. The terminal uses their dark palettes:

| Theme | ID | Palette |
| --- | --- | --- |
| Aurora | `aurora` | Ice blue and cyan |
| Orbit | `orbit` | Purple and rose |
| Graphite | `graphite` | Cool blue and blue-gray |
| Forest | `forest` | Soft green |

```bash
spaceprune theme
spaceprune theme list
spaceprune theme set orbit
spaceprune --config /absolute/path/to/config.toml theme set forest
```

`spaceprune theme` opens a preview: arrow keys or `1`–`4` switch themes and immediately redraw the full interface and status colors. Enter saves; Esc cancels. `theme set <ID>` sets directly; `theme list` lists themes and color samples read-only. `theme --output json` returns the current theme directly without opening the picker or changing configuration.

Themes cover command help/errors, human-readable output, progress, and interactive selection. JSON and redirected output remain plain text. The CLI theme lives in the application's `[theme]` section; saving preserves other `config.toml` contents and comments. Scripts can read current state with `theme list --output json`:

```toml
[theme]
id = "aurora"
```

The terminal theme is independent of any host application’s own appearance settings.

Old configurations still start directly: Tokyo Night and Nord map to Aurora; Catppuccin Mocha, Dracula, and Monokai to Orbit; VS Code Dark Modern, Light Modern, and Quiet Light to Graphite; Solarized Dark to Forest. Reading does not rewrite the old configuration; the next save uses the new four IDs. Old themes no longer appear in the picker.

## Configuration discovery

`--config <PATH>` selects application `config.toml`, not an individual Recipe file. Without it, CLI discovers the platform default and loads `official:minimal` when no user configuration exists. See [configuration discovery](CONFIGURATION.md#2-default-configuration-discovery) for locations, XDG precedence and the full directory layout.

## Recipe selection

```bash
spaceprune recipe
spaceprune recipe list
spaceprune recipe set personal
```

`spaceprune recipe` interactively previews and selects recipes under `recipes` following `<ID>/<ID>.toml`; Enter saves and Esc cancels. `recipe set <ID>` switches directly and accepts only recipes actually present under the current `recipes` root. `recipe list` shows names, entry paths, and active state. Switching updates only the application `recipe` field, not the cleanup recipe.

`recipe --output json` directly returns the current recipe ID, name, entry, and configuration path, without opening a picker or writing configuration. `recipe list --output json` returns the list; `recipe set <ID> --output json` explicitly switches and returns the result.

## Built-in minimal recipe

The repository retains one built-in cleanup recipe:

```text
recipes/official/minimal.toml  ->  official:minimal
```

The embedded namespace is one level deep, with no additional category directories. It provides predictable offline defaults:

| Platform | Basic task | Recommendation |
| --- | --- | --- |
| Linux / macOS / Android / iOS | Direct children of the current user's cache directory | Not modified for 30 days |
| Windows | Direct children of the current user's temporary directory | Not modified for 7 days |

`official:minimal` has no specific agent, programming-language, package-manager, snapshot-tool, or automation configuration. Users can invoke Snapper, Linux system caches, and other capabilities explicitly, but those are not in the default scope.

To retain the minimal recipe in a user recipe, include it explicitly in the entry file:

```toml
schema-version = 2
name = "Personal SpacePrune"

include = ["official:minimal", "./personal.generated.toml"]
```

Omitting `official:minimal` creates a fully independent recipe.

## Local recipes

SpacePrune uses a minimal built-in recipe plus explicit local extensions:

1. Only `official:minimal` is compiled into the binary.
2. `config.toml` selects one `recipes/<ID>/<ID>.toml` by ID.
3. The selected recipe may include local TOML files from its directory.
4. SpacePrune reads only local files explicitly referenced by the current recipe.

### Complete recipes

A recipe contains all UTF-8 configuration files under `recipes/<ID>/`. See [Recipe bundles](CONFIGURATION.md#14-recipe-bundles) for import, export and atomic replacement constraints. After importing on another computer, rescan to confirm current paths and matches.

## Write cleanup rules

Use the [Recipe authoring guide](RECIPE-GUIDE.md) for rules, variable mappings, includes and recommendations, and the [Configuration reference](CONFIGURATION.md) for complete fields and specialized handler options. Check matches first with `spaceprune scan --config <PATH> --output json --no-progress`, then open interactive selection. Do not validate a new rule by running cleanup.

## Automation

```toml
[automations.daily]
name = "Daily cleanup"
modules = ["cache"]
at = "03:00"
```

```bash
spaceprune automation list
spaceprune automation install daily
spaceprune automation run daily
spaceprune automation uninstall daily
```

Linux uses systemd user units, macOS uses LaunchAgent, and Windows uses current-user scheduled tasks. `official:minimal` defines no automation. Installed tasks reference application `config.toml` and resolve its current recipe on each run. A Core session loading only a recipe without an application entry cannot install an automation it cannot reload. The Core installation API requires the host to pass a CLI runner explicitly. CLI uses its own path; GUI hosts cannot use the GUI executable as the scheduled cleanup runner.

Background runs do not wait for passwords. Accessible recommended items are still processed; protected content remains. Run the same plan manually in a terminal, authorize, and rescan remaining content. In `automation run --output json`, `status` distinguishes `completed`, `completed_with_issues`, `needs_authorization`, `failed`, and `cancelled`. Only issue-free `completed` returns 0. Scan warnings, pending authorization, failures, and timeouts return 1; user cancellation returns 130. If stopped during scanning, `plan` and `execution` are null. Execution reports retain actual successes and processed size; pending authorization does not imply earlier work was rolled back. Ordinary `scan` partial-result exit behavior is unchanged.

## Built-in handlers and external plugins

Prefer `builtin.filesystem/paths` for ordinary paths. Use dedicated handlers when domain APIs, special grouping, or system commands are needed.

The complete statically registered capabilities are:

| Handler ID | Platform | Purpose |
| --- | --- | --- |
| `builtin.filesystem/paths` | All platforms | Ordinary files and directories constrained by root/include/exclude boundaries |
| `builtin.snapper/snapshots` | Linux | Snapper discovery, protection, grouping, and deletion |
| `builtin.system-linux/package-cache` | Linux | zypper, apt/apt-get, dnf, or pacman package caches |
| `builtin.system-linux/journal` | Linux | systemd journal vacuum |
| `builtin.system-linux/tmpfiles` | Linux | systemd-tmpfiles cleanup |
| `builtin.system-linux/coredumps` | Linux | coredumps under systemd-tmpfiles retention rules |
| `builtin.system-linux/go-modcache` | Linux | Go module cache |
| `builtin.developer-tools/go-modcache` | Linux | Go module cache |
| `builtin.developer-tools/go-buildcache` | Linux | Go build cache |
| `builtin.developer-tools/cargo-registry-cache` | Linux | Cargo registry downloads |
| `builtin.developer-tools/npm-cache` | Linux | npm content-addressed cache |
| `builtin.developer-tools/docker-build-cache` | Linux / Windows / macOS | Build cache |
| `builtin.developer-tools/docker-dangling-images` | Linux / Windows / macOS | Dangling images |
| `builtin.developer-tools/docker-unused-images` | Linux / Windows / macOS | Unused tagged images (manual-only) |
| `builtin.developer-tools/docker-stopped-containers` | Linux / Windows / macOS | Stopped containers (manual-only) |

`official:minimal` invokes only the filesystem handler; recipes must explicitly reference other handlers. See [Dedicated handlers](CONFIGURATION.md#12-specialized-handlers) for each handler's options, recommendation behavior, and unavailable semantics. System-cache scans start with current permissions and use the shared read-only service to authorize on demand. If subtrees remain unreadable, readable size is retained and marked as a lower bound. Scanning never performs cleanup.

Install external plugins from local directories:

```bash
spaceprune plugin catalog
spaceprune plugin disable builtin.snapper
spaceprune plugin enable builtin.snapper
spaceprune plugin install /path/to/plugin
spaceprune plugin list
spaceprune plugin inspect community.example
spaceprune plugin remove community.example
```

`plugin list` groups Built-in plugins and External plugins, with table columns for name, plugin ID, supported platforms, and enabled state. Snapper and Linux system cleanup are built-in plugins; Linux is their platform attribute, not a separate “platform capabilities” category. CLI and Desktop use the same catalog. Disabled or unsupported plugins remain visible with their actual state.

`plugin catalog --output=json` returns the unified catalog of built-in/external plugins, complete handler IDs, versions, platform support, and local enabled state. `plugin list/inspect --output=json` retains the original external-plugin metadata format. Reading the catalog does not start plugins or request system authorization. If an existing external installation claims a built-in ID, the catalog reports `id conflict` while retaining the real built-in capability. The conflicting package cannot run. `plugin remove <id>` removes only that external installation; new external installations with built-in IDs are rejected.

`plugin disable/enable <id>` affects subsequent operations. CLI and Desktop share `plugins.toml` under the user configuration directory; it neither rewrites recipes nor follows the recipe selected by `--config`. Without the file, all plugins are enabled. Ordinary scans skip disabled tasks; explicitly selecting one returns `plugin-disabled`. After a state change, loaded sessions and cleanup using old scans require reload and rescan. Saving the same state again does not invalidate them. Already-started operations are not forcibly terminated.

See [Plugin protocol](../reference/PLUGIN-PROTOCOL.md) for the process protocol.

When a filesystem task row is focused, such as one named `target`, Details shows its full configured root path, listing each root when there are several. File and directory cleanup items still show their own `Location`. Paths are display-only and do not change selection scope or execution plans.

Modules, projects/folders, and files use the theme's primary accent, secondary accent, and light text respectively. Each known logical size matches its row's name color. Selection and focus leave these hierarchy colors unchanged: checkboxes and the status column separately identify full, partial, empty, and unavailable states. Full-selection markers and status words are green; partial-selection markers are amber. Selection counts and known selected sizes on the right match the name and logical size on the left. Unknown sizes remain amber; unselected, unavailable, and not-applicable states use secondary text.

Details paths, selection counts, and known values follow the focused hierarchy level, while selection status words and reasons use the same status colors as the table. Field labels and explanations are subdued. The shortcut footer separates keys from actions; narrow screens prioritize main actions, with other shortcuts under `?`. Theme preferences are stored in application configuration, not in cleanup recipes.
