# Acceptance status

Local validation environment: Windows 11, NTFS on NVMe, PowerShell 7, and Rust 1.99 MSVC.

## Results

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
