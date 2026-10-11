# Unlinka

[English](README.md) | [简体中文](README.zh-CN.md)

A fast Windows deletion tool with a CLI and native Windows confirmation dialog. Unlinka supports permanent deletion, console confirmation, scan previews, bounded concurrency, cancellation, and failure summaries.

Deletion bypasses the Recycle Bin. Cancellation stops work that has not started; it cannot restore items already deleted.

## Usage

The examples below assume `unlinka` is available on your `PATH`. Unlinka targets Windows 10/11 and requires no administrator privileges or additional GUI runtime. An installer and an Explorer context-menu entry are not currently provided.

```powershell
unlinka --help

# Show the native Windows confirmation dialog; Cancel is the default button
unlinka -r .\node_modules

# Confirm in an interactive terminal
unlinka -r --confirm console .\node_modules

# Explicitly skip confirmation in a script
unlinka -r --yes .\node_modules

# Scan and count without modifying the target or requesting confirmation
unlinka -r --dry-run .\node_modules

# Delete multiple targets; the thread budget includes the scanner
unlinka -r --threads 8 .\one .\two

# Handle a filename that starts with a hyphen
unlinka -- -example
```

Ordinary directories require `-r`. Symbolic links, junctions, and other reparse points are treated as entries; their destinations are never recursively scanned. `--threads` accepts 1–64 and defaults to the smaller of the available CPU thread count and 4. A budget of 1 runs synchronously; larger budgets allocate one scanner thread and the remaining threads to deletion. Discovered files can be deleted while scanning continues, without waiting for a complete tree count.

If native confirmation fails, Unlinka reports an error. Select `--confirm console` explicitly to use terminal confirmation. Console mode requires interactive stdin and stderr, and an empty answer rejects deletion. `--yes` cannot be combined with an explicit `--confirm`.

Results and diagnostics go to stderr. Normal runs count successfully deleted files, directories, and link entries; dry runs count candidates discovered during scanning. The summary reports elapsed time and the total number of failures, with details for at most 100 failures. Press Ctrl+C to cancel execution; items already deleted cannot be recovered by Unlinka.

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

Unlinka does not include Recycle Bin deletion, a context menu, exact percentage progress, automatic retries, handling of blocking processes, or a mode that empties a directory while retaining it. Its main optimization target is large numbers of small files on local NTFS volumes. Other file systems and network paths have not been validated. The native dialog uses the system task dialog and may differ from Explorer's layout across Windows versions.

## Documentation

- [Architecture and development](docs/architecture.md): module responsibilities, deletion constraints, native confirmation, dependencies, and source checks.
- [Acceptance status](docs/acceptance.md): current results, verification gaps, benchmarks, and validation commands.
