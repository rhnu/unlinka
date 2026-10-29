# Acceptance status

Local validation environment: Windows 11, NTFS on NVMe, PowerShell 7, and Rust 1.99 MSVC.

## Installer, Shell, and i18n implementation

The workspace contains the current-user installer pipeline, classic Explorer COM adapter, maintenance/launcher, native desktop tasks/settings, and English/Chinese Fluent resources. Both unsigned and optionally signed installers supply classic menus on Windows 10 and Windows 11's Show more options. Signed builds validate Authenticode signatures in addition to the SHA-256, ACL, installation lease, ownership, and transaction checks used by both modes.

`cargo xtask pack` acquires a versioned Inno compiler automatically, verifies its archive and extracted cache, and compiles the installer without a separately installed compiler. On Rust 1.99, workspace/all-target compilation, Rusteward formatting and strict lint, all 90 pure unit tests, and the two generated icon checks passed. After the archive backend configuration was finalized, all 13 xtask unit tests passed again. Signed-metadata compilation of desktop and maintenance also passed using a compile-only Subject, without a certificate or signature operation.

A cold compiler cache downloaded and verified Tools.InnoSetup 7.1.0 automatically. Subsequent package builds reused the verified archive and extracted cache without downloading again. The final unsigned package is `target/packages/0.1.1.4/Unlinka-Setup-0.1.1.4-x64.exe` (3,413,038 bytes; SHA-256 `7ae81cd4d38774c581d5d4c93ff474493ed8656ae4833bd7db6e13e8b46c0ad0`). All three payload file hashes matched. Setup and the five product EXE/DLL version resources are `0.1.1.4`; SDK manifest extraction confirmed all four EXEs contain the same assembly version and Common Controls v6. Installed program icons are embedded resources. The installer and packaged applications were not executed.

Cache corruption is covered by pure hash/path checks and source inspection of the rebuild path; damaged-cache recovery and concurrent package builds were not exercised dynamically. Generated icon bytes remain identical to the existing common PNG and ICO because ZIP extraction explicitly uses the renderer's Rust compression backend.

Certificate-backed signing and real Windows installation, Explorer menus, native UI, upgrade/repair recovery, and occupied-DLL uninstall retry require manual Windows acceptance. Direct invocation of the installed maintenance EXE can keep it mapped during interrupted-upgrade recovery; use the current or newer Setup when recovery must replace the helper itself. This path remains unverified dynamically.

Use disposable Windows test environments to check:

- Unsigned current-user installation and one classic entry on Win10 / under Show more options on Win11.
- Optional signed installation, signer Subject matching, and unchanged menu behavior.
- Mixed selections, raw UTF-16/long paths, shortcuts and reparse points, both languages, default cancellation, progress cancellation, replaced targets, and partial failures.
- Retained settings, same-version repair, cross-version upgrades, interrupted maintenance, external PATH/registry changes, and preserved unknown files.
- Both uninstall confirmations, retained uninstall entry on pending removal, and successful retry after occupied DLLs are released.

## CLI confirmation

CLI deletion uses terminal confirmation; native confirmation, progress, and result dialogs belong to Explorer Shell tasks. `--yes` / `-y` skips confirmation, and `--dry-run` scans without confirmation or deletion. Pure unit tests cover affirmative and negative answers, empty answers, EOF, output failure, complete local/UNC path display, and both yes switches.

Current presentation uses concise permanent-deletion prompts, explicit complete/incomplete/cancelled outcomes, localized common Windows errors, and a 5-failure terminal preview with `-v` for retained details. Native targets and failures use a fixed-size WinSafe scrollable text viewer. The terminal checks below predate this presentation adjustment; the refactored viewer's layout, copy behavior, focus return, repeated opening, and DPI behavior still require manual Windows acceptance.

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

Native UI checks are manual and are not covered by the Cargo integration tests. File and directory symbolic-link deletion remains unverified locally because the account lacks creation privileges. Other Windows builds, other file systems, and network paths remain unverified. Concurrent modification limits are described in the [architecture](architecture.md).

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
- Closing details with Enter, Escape, or the window close control returns to the unchanged confirmation dialog without deleting anything; repeat opening and closing and verify focus returns to its previous control.
- Failure totals and the first path/reason remain visible; View failure details shows retained paths/reasons and the omitted count without expanding the result dialog.
- Cancellation keeps progress counts visible and closes after active operations return; check both languages and 100% / 150% / 200% display scaling, including opening details on another monitor, dragging the viewer between monitors with different DPI, and changing scaling while it is open.
- Delete removes the selected targets.
- Replacing a target while the dialog is open rejects execution and preserves both objects.
