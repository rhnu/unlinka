# Unlinka

[English](README.md) | [简体中文](README.zh-CN.md)

A fast Windows deletion tool with a bilingual CLI, native task dialogs, and Explorer context menus. Unlinka supports permanent deletion, console confirmation, scan previews, bounded concurrency, cancellation, and failure summaries.

Deletion bypasses the Recycle Bin. Cancellation stops work that has not started; it cannot restore items already deleted.

## Installation

The x64 current-user installer targets Windows 10/11 without administrator privileges. The default unsigned build installs the classic Explorer menu: directly on Windows 10 and under **Show more options** on Windows 11. Menus and user PATH are enabled by default. Signing is optional and does not change menu behavior. The fixed installation root is `%LocalAppData%\Programs\Unlinka`; the Start menu shortcut opens language, menu, and PATH settings. Menu actions always open a confirmation dialog, then show progress, cancellation, and results.

Installer, repair, upgrade, and uninstall code is implemented. Unsigned builds are installable; signed builds additionally validate Authenticode signatures. Signed and unsigned builds share the same product identity and installation. Certificate-backed signing and real Windows lifecycle/menu acceptance remain pending. See [integration and packaging](docs/windows-integration.md) for signing options and remaining validation.

Download unsigned x64 installers from [GitHub Releases](https://github.com/RhNu/unlinka/releases). Pushing `v<major.minor.patch>` or `v<major.minor.patch>.<build-number>` runs the shared checks and publishes an installer plus its SHA-256 checksum. The first three components must match the Cargo version; an omitted build number is `0`. The [Release workflow](https://github.com/RhNu/unlinka/actions/workflows/release.yml) also supports manual builds that upload an Actions artifact without publishing a release.

Build the default installer with Rust 1.99 MSVC and Windows SDK x64 build tools. The first build downloads and verifies the Inno compiler; later builds verify and reuse its local cache. No separate Inno installation or compiler path is needed:

```powershell
cargo xtask pack --build-number 2
```

Outputs are immutable under `target/packages/<four-part-version>`; use a new build number for each package. Setup is named `Unlinka-Setup-<four-part-version>-x64.exe` for both signing modes. To enable signing, supply `--publisher`, `--certificate-thumbprint`, and `--timestamp-url` together; Publisher must exactly match the certificate Subject. The payload's `unlinka.exe` also runs as a portable CLI. Installed upgrades preserve settings and retain the preceding payload for rollback. Uninstall preserves preferences and logs unless selected otherwise; occupied DLLs leave a reported pending state and a usable uninstall entry for retry.

## Usage

The examples below assume `unlinka` is available on your `PATH`. No additional GUI runtime is required.

```powershell
unlinka --help

# Localized help; auto follows Windows display-language preferences
unlinka --lang zh-CN --help

# Query installation health without starting deletion
unlinka --integration-status --json

# Confirm interactively in the terminal; an empty answer cancels
unlinka -r .\node_modules

# Explicitly skip confirmation in a script
unlinka -r --yes .\node_modules

# Scan and count without modifying the target or requesting confirmation
unlinka -r --dry-run .\node_modules

# Language-independent, versioned JSON for scripts; confirmation rules still apply
unlinka -r --yes --json .\node_modules

# Delete multiple targets; the thread budget includes the scanner
unlinka -r --threads 8 .\one .\two

# Handle a filename that starts with a hyphen
unlinka -- -example
```

Ordinary directories require `-r`. Symbolic links, junctions, and other reparse points are treated as entries; their destinations are never recursively scanned. `--threads` accepts 1–64 and defaults to the smaller of the available CPU thread count and 4. A budget of 1 runs synchronously; larger budgets allocate one scanner thread and the remaining threads to deletion. Discovered files can be deleted while scanning continues, without waiting for a complete tree count.

CLI deletion always uses terminal confirmation and requires interactive stdin and stderr. Only `y` or `yes` confirms deletion (case-insensitive); an empty answer cancels. Use `--yes` / `-y` to skip confirmation in scripts, or `--dry-run` to scan without deleting. Native confirmation, progress, and result dialogs are used for Explorer Shell actions. **View targets** and **View failure details** open a fixed-size, scrollable text viewer with complete paths and selectable text, keeping the main dialog's size stable.

Results and diagnostics go to stderr. Normal runs count successfully deleted files, directories, and link entries; dry runs count candidates discovered during scanning. The summary distinguishes completion, incomplete execution, and cancellation, and reports elapsed time and the total number of failures. Ordinary terminal output shows up to 5 failures as path and reason; `-v` shows up to 100 retained failures with operations and error codes. Shell details and JSON retain up to 100 failures; omitted failures are counted explicitly. Press Ctrl+C to cancel execution; items already deleted cannot be recovered by Unlinka.

Language selection is `--lang` → `UNLINKA_LANG` → saved preference → Windows display languages → English. Supported values are `auto`, `zh-CN`, and `en-US`. JSON results use schema version 1, stable diagnostic codes, numeric counts, and raw UTF-16 path arrays; final non-help results write one document to stdout. `--integration-status` is incompatible with deletion arguments.

| Exit code | Meaning                                                                |
| --------- | ---------------------------------------------------------------------- |
| 0         | Completed without failures, or the dry-run scan completed              |
| 1         | Execution failed or completed with partial failures                    |
| 2         | Invalid arguments or target preflight failed; deletion has not started |
| 3         | Cancelled by the user; execution may already have deleted some items   |
| 4         | Confirmation dialog or input failed; deletion has not started          |

## Limits

Unlinka does not change read-only attributes, elevate privileges, or terminate processes holding files open. It revalidates all top-level targets before execution and aborts the entire batch if any target cannot be opened or its identity has changed. Permission and sharing conflicts during execution are reported as failures while other independent items continue. Protected paths include volume roots, the Windows directory and its contents, the user directory, the current working directory, and relevant ancestors. These protections do not replace backups of other important data.

Preflight records target identities, which are checked again by reopening the targets before execution. During scanning, directory handles restrict ordinary writes and renames, and directory types are revalidated; sharing conflicts are reported rather than ignored. Newly created files can cause a directory-not-empty failure. If a file is replaced before it is opened, the replacement at the same path may still be deleted. Leaf entries are deleted through open handles, so an already selected object is still processed if another process subsequently moves it outside the selected directory. Attribute-only changes to reparse points still have a race window between validation and opening. Unlinka does not provide isolation against malicious concurrent changes.

Unlinka does not include Recycle Bin deletion, exact percentage progress, automatic deletion retries, handling of blocking processes, or a mode that empties a directory while retaining it. Shell integration targets native x64 Explorer; ARM64/x86 and directory-background menus are outside this release. Its main optimization target is large numbers of small files on local NTFS volumes. Other file systems and network paths have not been validated. The native dialog uses the system task dialog and may differ from Explorer's layout across Windows versions.

## Documentation

- [Architecture and development](docs/architecture.md): module responsibilities, deletion constraints, native confirmation, dependencies, and source checks.
- [Acceptance status](docs/acceptance.md): current results, verification gaps, benchmarks, and validation commands.
- [Windows integration and i18n](docs/windows-integration.md) (Chinese): installation layout, classic menu registration, lifecycle recovery, packaging, and remaining acceptance.
