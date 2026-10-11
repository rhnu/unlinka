# Acceptance status

Local validation environment: Windows 11, NTFS on NVMe, PowerShell 7, and Rust 1.99 MSVC.

## Installer, Shell, and i18n implementation

The workspace contains the current-user installer pipeline, classic/modern COM adapters, maintenance/launcher, native desktop tasks/settings, and English/Chinese Fluent resources. Packaging now uses one product identity and install location. The default unsigned installer is installable and supplies classic menus on Windows 10 and Windows 11's Show more options. Providing all three certificate signing arguments also builds the sparse MSIX and enables the modern Windows 11 menu when menus are enabled.

Payload schema 1 records a `signed` boolean. Signed payloads require signature validation; both modes retain SHA-256 checks, request ACLs, installation leases, ownership checks, and transaction recovery. Unsigned payloads contain neither an MSIX nor an embedded desktop package identity. Output paths and Setup names are identical across signing modes, and existing package versions cannot be overwritten.

Source validation passed: workspace/all-target compilation in both unsigned and signed-metadata configurations, Rusteward formatting and strict lint, and all 66 pure unit tests. The signed-metadata check compiled the package-identity branch without using a certificate.

The default unsigned release installer was built as `target/packages/0.1.0.6/Unlinka-Setup-0.1.0.6-x64.exe` (3,351,234 bytes; SHA-256 `3740ef5f2cfac235e17eca9ed13e809a27ebe9e591f4bd8fa0d13bc332871557`). All 71 payload inventory hashes matched. Metadata records `signed: false` and an empty Publisher; no MSIX was generated. The five application binaries and Setup report `NotSigned` and product version `0.1.0.6`. Extraction of the desktop executable's embedded manifest confirmed no MSIX identity and retained Common Controls. The packaged CLI's Chinese help also ran successfully.

Certificate-backed signing has not been exercised with a certificate. No product installer, package registration, Explorer action, native UI, actual deletion, or benchmark was executed for these changes. The existing CI deletion test definitions remain opt-in locally and use deterministic English or stable JSON output.

Real Win10/Win11 menu behavior, package identity/deployment, installer selections, settings dialogs, busy-task maintenance, cross-version rollback, crash recovery, and occupied-DLL uninstall retry still require Windows acceptance. Do not infer these results from pure unit tests or the historical deletion results below. Rust 1.85 has not been validated locally.

Use disposable Windows test environments for the unified installation identity:

- Confirm an unsigned fresh install succeeds without a certificate, creates no identity package, and provides one classic entry on Win10 and under Show more options on Win11.
- Confirm a signed build retains the classic entry and adds one modern entry on Win11; check exact Publisher, package version, architecture, and external location.
- Test mixed selections, raw UTF-16/long paths, shortcuts, and reparse points; test both languages, default cancellation, progress cancellation, changed targets, and partial failures using disposable fixtures.
- Exercise retained settings, same-version repair, cross-version upgrades in both signing directions, package failure rollback, interrupted maintenance, external PATH/registry changes, and preserved unknown files.
- Cancel both uninstall confirmations before cleanup; retain the uninstall entry on pending removal, release occupied DLLs, and retry successfully.

## Historical deletion-engine results

The following results predate the installer/Shell/i18n changes and were not rerun for this implementation.

| Check                | Status                                                                                                                             |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Source checks        | Compilation, formatting, strict lint, release build, and 22 unit tests passed                                                      |
| Deletion acceptance  | All 8 grouped tests passed; symbolic-link subcases skipped for privilege error 1314                                                |
| Preview and counts   | Dry runs preserve contents and attributes; deletion counts match actual results with 1 / 8 threads                                 |
| Paths and links      | Unicode, spaces, long paths, junctions, and hard links passed; junction destinations remain intact                                 |
| Partial failures     | Read-only and open files remain intact; independent deletions continue; failure details are capped at 100                          |
| Input protection     | Duplicate / overlapping targets, recursion requirements, whole-batch validation, and working-directory protection passed           |
| Console cancellation | Real `CTRL_BREAK_EVENT` checks passed with 1 / 8 threads: exit code 3, accurate counts, no failures, and preserved remaining files |
| Native confirmation  | Manual checks passed for full-path display, multi-target deletion, default Enter, Escape, and window-close cancellation            |
| Target replacement   | Replacing a target while confirmation is open rejects execution and preserves both objects                                         |
| Benchmark            | All 30 samples passed: 12,000 files per sample, correct counts, preserved sentinels, and fixture cleanup                           |

Native UI checks are manual and are not covered by the Cargo integration tests. File and directory symbolic-link deletion remains unverified locally because the account lacks creation privileges. Rust 1.85, other Windows builds, other file systems, and network paths remain unverified. Concurrent modification limits are described in the [architecture](architecture.md).

## Performance

The benchmark compares Unlinka with 1 / 4 / 8 threads, .NET `Directory.Delete`, and PowerShell `Remove-Item`. Each method runs three rounds on flat and tree fixtures containing 12,000 files of 1 KiB, with warm metadata. Four threads have the lowest median deletion time in both shapes on this machine, supporting the default cap of 4.

Results cover local NTFS / NVMe small-file fixtures only. Cold caches, mechanical disks, network shares, and real dependency directories are unmeasured. JSON reports under `.scratch/benchmark-results-*.json` retain timings, executable identity, environment, and methodology.

## Validation commands

Run from the repository root on Windows MSVC. Cargo builds the tested application; no `PATH` installation is needed. Deletion checks are opt-in and use disposable fixtures with path containment and cleanup.

```powershell
cargo test -p unlinka --test acceptance --release --locked -- --ignored --test-threads=1 --nocapture
cargo test -p unlinka --test cancellation --release --locked -- --ignored
cargo xtask benchmark --file-count 12000 --rounds 3 --shapes flat,tree
```

Without `--ignored`, the integration targets skip actual deletion. The cancellation harness uses an isolated signal helper to preserve the user's terminal. [CI](../.github/workflows/ci.yml) runs acceptance and cancellation sequentially after source checks and unit tests; benchmarks run manually.

The benchmark builds a release binary unless `--binary <path>` selects one. Baselines require `pwsh`. Deletion-phase timing excludes startup; process timing includes it. Run benchmarks separately from other I/O checks.

For manual native-dialog validation, check that:

- Default Enter, Escape, and closing the window preserve the targets.
- Expanded paths are complete, including long paths, combining characters, and joined emoji.
- Delete removes the selected targets.
- Replacing a target while the dialog is open rejects execution and preserves both objects.
