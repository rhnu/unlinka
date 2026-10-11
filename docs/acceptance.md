# Acceptance status

Local validation environment: Windows 11, NTFS on NVMe, PowerShell 7, and Rust 1.99 MSVC.

## Installer, Shell, and i18n implementation

The workspace contains the current-user installer pipeline, classic/modern COM adapters, maintenance/launcher, native desktop tasks/settings, and English/Chinese Fluent resources. Packaging now uses one product identity and install location. The default unsigned installer is installable and supplies classic menus on Windows 10 and Windows 11's Show more options. Providing all three certificate signing arguments also builds the sparse MSIX and enables the modern Windows 11 menu when menus are enabled.

Payload schema 1 records a `signed` boolean. Signed payloads require signature validation; both modes retain SHA-256 checks, request ACLs, installation leases, ownership checks, and transaction recovery. Unsigned payloads contain neither an MSIX nor an embedded desktop package identity. Output paths and Setup names are identical across signing modes, and existing package versions cannot be overwritten.

Current source validation passed: workspace/all-target compilation, Rusteward formatting and strict lint, and all 82 pure unit tests. Tests cover retained menu preferences, durable uninstall purge choices, legacy transaction metadata, rejected PATH intents, packaging compiler-flag precedence, concise report boundaries, outcome precedence, localized Windows errors, and JSON-based validation counts. Signed-metadata compilation of desktop and maintenance packages previously passed without using a certificate; that configuration was not rerun for the presentation adjustment.

The default unsigned release installer was built as `target/packages/0.1.0.8/Unlinka-Setup-0.1.0.8-x64.exe` (3,454,089 bytes; SHA-256 `ddd3cc8aaa3dceed91276e498376be3c09b659a907fc158cabbbb6bfe01326e4`). All 71 payload inventory hashes matched; metadata records `signed: false`, and no MSIX was generated. SDK `mt.exe` extraction confirmed all four executable manifests contain Common Controls v6 and version `0.1.0.8`, without MSIX identity. PE import inspection confirmed all five product EXE/DLL files have no dynamic VC/UCRT dependency. The packaged maintenance executable's help and CLI's Chinese help started successfully without installing the product.

The installer audit fixed missing maintenance/desktop Common Controls selection, undeployed dynamic CRT dependencies, UTF-8 resource-script paths, fixed-directory overrides, early payload publication before Inno's uninstall entry exists, false menu-preference loss, failed-uninstall retry entry restoration, durable purge choices, rejected PATH intents, MSIX name casing, and misleading lock-error classification. Duplicate manifest build logic, repeated uninstall confirmation, and repeated Setup signature verification were removed. Settings now verify and run an external maintenance copy directly. Direct invocation of the installed maintenance EXE can still keep that EXE mapped during interrupted-upgrade recovery; use the current or newer Setup when recovery must replace the helper itself. This remaining path has not been verified dynamically.

Certificate-backed signing has not been exercised with a certificate. No product installer, package registration, Explorer action, native UI, or benchmark was executed for the installer changes. CLI deletion checks for the current confirmation behavior are recorded below. The existing CI deletion test definitions remain opt-in locally and use deterministic English or stable JSON output.

Real Win10/Win11 menu behavior, package identity/deployment, installer selections, settings dialogs, busy-task maintenance, cross-version rollback, crash recovery, and occupied-DLL uninstall retry still require Windows acceptance. Do not infer these results from pure unit tests or the historical deletion results below. Rust 1.85 has not been validated locally.

Use disposable Windows test environments for the unified installation identity:

- Confirm an unsigned fresh install succeeds without a certificate, creates no identity package, and provides one classic entry on Win10 and under Show more options on Win11.
- Confirm a signed build retains the classic entry and adds one modern entry on Win11; check exact Publisher, package version, architecture, and external location.
- Test mixed selections, raw UTF-16/long paths, shortcuts, and reparse points; test both languages, default cancellation, progress cancellation, changed targets, and partial failures using disposable fixtures.
- Exercise retained settings, same-version repair, cross-version upgrades in both signing directions, package failure rollback, interrupted maintenance, external PATH/registry changes, and preserved unknown files.
- Cancel both uninstall confirmations before cleanup; retain the uninstall entry on pending removal, release occupied DLLs, and retry successfully.

## CLI confirmation

CLI deletion uses terminal confirmation; native confirmation, progress, and result dialogs belong to Explorer Shell tasks. `--yes` / `-y` skips confirmation, and `--dry-run` scans without confirmation or deletion. Pure unit tests cover affirmative and negative answers, empty answers, EOF, output failure, complete local/UNC path display, and both yes switches.

Current presentation uses concise permanent-deletion prompts, explicit complete/incomplete/cancelled outcomes, localized common Windows errors, and a 5-failure terminal preview with `-v` for retained details. Native targets and failures use a fixed-size scrollable text viewer. The terminal checks below predate this presentation adjustment; the new viewer's layout, copy behavior, focus return, and DPI behavior still require manual Windows acceptance.

Real Windows terminal validation passed using the rebuilt debug CLI in an interactive pseudoterminal, with one fresh disposable file per case:

| Case                   | Exit code | File state and terminal behavior                                     |
| ---------------------- | --------- | -------------------------------------------------------------------- |
| Press Enter at `[y/N]` | 3         | Target retained with its original content; cancellation reported     |
| Answer `y`             | 0         | Target deleted; summary reported one file and zero failures          |
| Answer `yes`           | 0         | Target deleted; summary reported one file and zero failures          |
| Pass `--yes`           | 0         | Target deleted without a prompt; one file and zero failures reported |
| Pass `-y`              | 0         | Target deleted without a prompt; one file and zero failures reported |

An outside-target sentinel retained its original content throughout, and the owned temporary directory was removed after validation. Explorer interactions were not exercised for this adjustment. Remaining checks with disposable targets:

- Answer `n` at the terminal prompt and verify that the targets remain.
- Redirect stdin or stderr without a skip option and verify confirmation error status 4 with targets preserved; verify `--dry-run` preserves them without confirmation.
- Use the Explorer context menu and check native confirmation, progress, cancellation, and results.

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
- View targets opens a bounded scrollable window with complete paths, including long paths, combining characters, and joined emoji; copying does not add hard-wrapped line breaks.
- Closing details with Enter, Escape, or the window close control returns to the unchanged confirmation dialog without deleting anything.
- Failure totals and the first path/reason remain visible; View failure details shows retained paths/reasons and the omitted count without expanding the result dialog.
- Cancellation keeps progress counts visible and closes after active operations return; check both languages and 100% / 150% / 200% display scaling.
- Delete removes the selected targets.
- Replacing a target while the dialog is open rejects execution and preserves both objects.
