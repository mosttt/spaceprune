# SpacePrune Core / CLI

This public MIT repository contains the CLI, reusable Core libraries and built-in plugins.
Start at `README.md` and `docs/en/build/DEVELOPMENT.md`.

## Working rules

- Keep filesystem, developer-tool and platform lifecycle responsibilities separate. Docker cleanup is desktop-platform only.
- Preserve trusted native authorization, exact confirmed selections and partial-effect reporting. Never bypass signature checks for a missing Windows component.
- Read affected code, callers, tests and existing changes before editing. Prefer direct, maintainable changes and targeted tests.
- Test scan/cleanup only on disposable fixtures. Native Windows/macOS acceptance is separate from cross-compilation.
- Keep public documentation about supported behavior, architecture, building and genuine acceptance gaps. Do not include conversations, private-product source/layout, operator handoffs, personal paths or unpublished candidate archives.
- Default `README.md` is English with a `README.zh-CN.md` language link. Language-specific documentation stays in `docs/en/` and `docs/zh/`. Keep comments in English.
- Never commit credentials, private keys, local configuration, build outputs or caches. Preserve required third-party licenses and source materials.
- Use the configured `origin` for authorized normal commits and fast-forward pushes. History rewrites, Git deletion/reinitialization and publishing elsewhere require explicit authorization.
