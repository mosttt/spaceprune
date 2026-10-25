# Windows system locations and path representations

`spaceprune-windows-elevation::SystemLocations` provides a read-only snapshot of system locations. Callers can query the system volume, Windows directory, and protected system directories; the object grants neither read nor deletion permission.

`query()` uses `GetSystemWindowsDirectoryW` and Known Folder APIs for configured Windows, Program Files, Program Files (x86), and ProgramData locations. `GetVolumePathNameW` identifies the system volume. Native APIs and canonical filesystem paths establish actual directories and volume GUID aliases. The implementation does not read `SystemDrive`, `SystemRoot`, or `ProgramFiles` environment variables directly, or assume Windows is at `C:\Windows`.

`is_system_volume()` matches only the system volume root, not ordinary subdirectories on the same drive. `is_protected()` operates on canonical paths produced by scanning and matches directory boundaries: `Windows\System32` is inside Windows, but `Windows.old` is not its descendant. Configured Program Files or ProgramData directories on other drives are also protected.

Comparison preserves raw UTF-16 units and supports ordinary DOS, extended DOS, volume GUID, and UNC representations. Case comparison uses Windows `CompareStringOrdinal`; lossy display strings are not used for path decisions. Unknown device namespaces, relative paths, NUL, ADS, and noncanonical paths containing `.` or `..` cannot be classified as ordinary processable paths on this basis.

Configured directories may pass through junctions. Existing canonical paths are resolved before volume aliases are established, while the configured spelling remains protected too. String/directory classification does not replace execution-time handle, volume/file identity, permission, and scope checks.

If required system locations or volume mappings cannot be established, `query()` returns `None`. Callers should report unavailable locations, not fall back to mutable environment variables to guess system directories. Callers may cache a successful snapshot, but must not treat it as cached authorization or permanently cache query failures.

Portable tests cover path representations, directory separators and boundaries, drive/volume distinctions, abnormal namespaces, and UTF-16 preservation. Windows-specific tests also cover actual OS locations, volume GUID/canonical paths, Unicode case comparison, directory relocation, and environment-variable interference in a separate process. Cross-compilation does not establish that those native Windows cases have run.
