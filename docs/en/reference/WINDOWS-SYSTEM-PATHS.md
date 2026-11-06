# Windows system locations and path representations

`spaceprune-windows-elevation::SystemLocations` provides a read-only snapshot of system locations. Callers can query the system volume, Windows directory, and protected system directories; the object grants neither read nor deletion permission.

`query()` uses `GetSystemWindowsDirectoryW` and Known Folder APIs for configured Windows, Program Files, Program Files (x86), and ProgramData locations. `GetVolumePathNameW` identifies the system volume. Native APIs and canonical filesystem paths establish actual directories and volume GUID aliases. The implementation does not read `SystemDrive`, `SystemRoot`, or `ProgramFiles` environment variables directly, or assume Windows is at `C:\Windows`.

`is_system_volume()` matches only the system volume root, not ordinary subdirectories on the same drive. `is_protected()` operates on canonical paths produced by scanning and matches directory boundaries: `Windows\System32` is inside Windows, but `Windows.old` is not its descendant. Configured Program Files or ProgramData directories on other drives are also protected.

Comparison preserves raw UTF-16 units and supports ordinary DOS, extended DOS, volume GUID, and UNC representations. Case comparison uses Windows `CompareStringOrdinal`; lossy display strings are not used for path decisions. Unknown device namespaces, relative paths, NUL, ADS, and noncanonical paths containing `.` or `..` cannot be classified as ordinary processable paths on this basis.

Configured directories may pass through junctions. Existing canonical paths are resolved before volume aliases are established, while the configured spelling remains protected too. String/directory classification does not replace execution-time handle, volume/file identity, permission, and scope checks.

If required system locations or volume mappings cannot be established, `query()` returns `None`. Callers should report unavailable locations, not fall back to mutable environment variables to guess system directories. Callers may cache a successful snapshot, but must not treat it as cached authorization or permanently cache query failures.

Portable tests cover path representations, directory separators and boundaries, drive/volume distinctions, abnormal namespaces, and UTF-16 preservation. Windows-specific tests also cover actual OS locations, volume GUID/canonical paths, Unicode case comparison, directory relocation, and environment-variable interference in a separate process. Cross-compilation does not establish that those native Windows cases have run.

## Windows maintenance plugins

`builtin.system-windows/recycle-bin` enumerates the current user's Shell Recycle Bin. Cleanup binds each confirmed item's Shell identity, owner and timestamps, re-enumerates before removal, and permanently deletes only that item. Restored, changed or newly recycled objects are not substituted. A failed operation can have partial effects and is not retried automatically.

`builtin.system-windows/component-store` delegates superseded Windows update/component cleanup to the system DISM `/Online /Cleanup-Image /StartComponentCleanup /NoRestart` operation. It uses the signed cleanup component (capability protocol 6) when UAC is needed. It never uses `/ResetBase`, stops update services, or deletes SoftwareDistribution, WinSxS or CbsTemp as files. Size is not applicable to the maintenance action; a restart may be required. It does not empty all downloaded updates. Both handlers default to optional selection.

`builtin.memory` is an explicit Windows working-set operation, separate from Recipes and disk-space totals. Only current-user, noncritical processes that can be opened are listed. Selection binds PID and creation time; the process handle remains open during `K32EmptyWorkingSet`. No process is terminated and no administrator prompt is requested. The reported change is an observed working-set reduction, not guaranteed freed physical RAM; applications can immediately load pages again.

`SystemLocations::is_reviewable_cache` identifies only Windows Temp, SystemTemp and ProgramData ChocolateyHttpCache under OS-owned roots. This classification grants no access and does not bypass canonical identity or reparse-point checks. All other Windows servicing directories remain protected.
