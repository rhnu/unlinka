# Architecture

Unlinka is a Windows-only Rust workspace with a CLI, an independent deletion engine, and native confirmation.

## Responsibilities

| Package / module          | Responsibility                                                                            |
| ------------------------- | ----------------------------------------------------------------------------------------- |
| `modules/app` / `cli`     | Arguments, thread budget, and exit status                                                 |
| `modules/app` / `main`    | Preflight → confirmation → execution → summary, plus cancellation                         |
| `modules/app` / `confirm` | Native TaskDialog and console confirmation                                                |
| `modules/core` / `paths`  | Path resolution, input protection, target identity, and duplicate / overlap normalization |
| `modules/core` / `engine` | Bounded scheduling, scanning, deletion, directory completion, and summaries               |
| `modules/core` / `win`    | UTF-16, handles, reparse-point handling, enumeration, and deletion APIs                   |
| `modules/validation`      | Disposable fixtures, path containment, cleanup, and child-process utilities               |
| `modules/xtask`           | Benchmarks, baseline comparison, and JSON reports                                         |

The core is independent of the CLI and UI. Callers use `prepare`, display its returned targets, then pass `PreparedTargets` to `execute`. Invalid input prevents the entire batch from starting; execution can partially succeed.

## Deletion and concurrency

The thread budget is 1–64 and includes the scanner; the default is the smaller of available CPU threads and 4. One thread runs synchronously. Larger budgets use one scanner and the remaining threads for deletion through a bounded channel. Directories are deleted after their enumerated children finish. Confirmation shows top-level targets without scanning for an exact recursive count.

Cancellation stops operations that have not started; active system calls must return. Success counts reflect completed operations. Dry runs traverse without deleting. Failure details are capped at 100 while the total count remains accurate.

Reparse points are never recursively traversed. Preflight records each top-level target's volume identity, file identity, and entry type; all targets are reopened and checked before execution. Scanned directories and ancestors retain handles that deny write / delete sharing. Child tasks keep required parent handles alive. Internal directory types are revalidated under a read lock; completion closes handles under a write lock.

Leaf handles allow delete sharing for hard links. Deletion stays bound to the opened object even if another process moves it outside the selected directory. A replacement made before opening may be deleted. Attribute-only changes can bypass sharing restrictions, leaving a race between enumeration, validation, and opening. The engine does not isolate malicious concurrent changes.

Deletion uses `SetFileInformationByHandle(FileDispositionInfoEx)` with POSIX semantics, falling back to ordinary handle-based deletion when unsupported. Permission and sharing failures are reported; read-only attributes are preserved. Successful entry deletion does not guarantee immediate handle invalidation or disk-space reclamation. See Microsoft's [deletion API](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle) and [sharing rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).

## Native confirmation

`TaskDialogIndirect` shows a warning, Delete, and Cancel. Cancel is the default; Escape and window close also cancel. Single targets show name and location, multiple targets show the top-level count, and expanded details show full paths. Content sizing and grapheme-based wrapping preserve long paths, combining characters, and joined emoji. Execution uses the original paths. The dialog explains permanent deletion and cancellation limits.

`build.rs` embeds a Common Controls v6 manifest with current-user privileges, long-path support, and DPI awareness. Dialog failure returns a confirmation error; console confirmation requires explicit selection. See Microsoft's [task dialog documentation](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-taskdialogindirect).

## Dependencies

Runtime dependencies provide Win32 bindings (`windows-sys`), argument parsing (`clap`), bounded channels (`crossbeam-channel`), cancellation (`ctrlc`), and grapheme segmentation (`unicode-segmentation`). Benchmark tooling uses `serde`, `serde_json`, and `sha2` for reports and hashes. Versions are declared in [Cargo.toml](../Cargo.toml) and resolved in [Cargo.lock](../Cargo.lock).

## Development and validation

Builds target Windows 10/11 with MSVC. The declared minimum Rust version is 1.85; local validation uses Rust 1.99. Validation results and remaining gaps are in [acceptance status](acceptance.md).

```powershell
cargo check --workspace --all-targets --locked
cargo dev format --check --diff --locked
cargo dev lint --locked
cargo test --workspace --lib --bins --locked
cargo build --release --locked
```

`cargo dev` uses [Rusteward](https://github.com/RhNu/Rusteward). The test command runs unit tests; ordinary `cargo test --workspace` skips opt-in deletion checks. Integration and benchmark commands and manual UI checks are documented in [acceptance status](acceptance.md). [`.gitattributes`](../.gitattributes) keeps Rust sources in LF form.

### Continuous integration

[CI](../.github/workflows/ci.yml) runs on pushes, pull requests, and manual dispatch. The Windows job uses stable MSVC, rustfmt, Clippy, and Rusteward for compilation, formatting, lint, and unit tests, followed by sequential release-mode acceptance and cancellation tests. Acceptance uses one test thread and reports symbolic-link privilege skips; cancellation checks 1 / 8 deletion threads. Cargo commands use `--locked`. Benchmarks run manually.

### Markdown maintenance

Local checks and CI share [`.rumdl.toml`](../.rumdl.toml): `.gitignore` is respected, MD013 is disabled, and MD060 requires aligned tables. CI uses rumdl 0.2.79 and fails on configuration warnings.

```powershell
uv tool install rumdl==0.2.79

# Fix only files changed in the task, then review the diff
rumdl check --fix README.md docs/architecture.md

# Check all documentation without rewriting
rumdl check --deny-config-warnings
```
