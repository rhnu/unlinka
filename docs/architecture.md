# Architecture

Unlinka is a Windows-only Rust workspace with a CLI, an independent deletion engine, native task dialogs, two Explorer COM adapters, and a current-user installer.

[Windows integration and i18n](windows-integration.md) defines the implemented installation layout, registration contracts, lifecycle recovery, and packaging. Release signing and real Windows menu/lifecycle acceptance remain pending.

## Responsibilities

| Package / module          | Responsibility                                                                                |
| ------------------------- | --------------------------------------------------------------------------------------------- |
| `modules/app` / `cli`     | Arguments, thread budget, and exit status                                                     |
| `modules/app` / `main`    | Preflight → confirmation → execution → summary, plus cancellation                             |
| `modules/app` / `confirm` | Native TaskDialog and console confirmation                                                    |
| `modules/app` / `result`  | Versioned JSON results, stable diagnostics, and raw UTF-16 paths                              |
| `modules/core` / `paths`  | Path resolution, input protection, target identity, and duplicate / overlap normalization     |
| `modules/core` / `engine` | Bounded scheduling, scanning, deletion, directory completion, and summaries                   |
| `modules/core` / `win`    | UTF-16, handles, reparse-point handling, enumeration, and deletion APIs                       |
| `modules/i18n`            | Embedded Fluent resources, locale negotiation, and formatting                                 |
| `modules/integration`     | Installation leases, requests, registry/PATH/package ownership, transactions, and maintenance |
| `modules/launcher`        | Stable installed CLI entry, lossless argument forwarding, and child exit/cancellation         |
| `modules/desktop`         | Native confirmation, progress, results, and staged settings                                   |
| `modules/shell`           | Classic `IContextMenu`, modern `IExplorerCommand`, selection adapters, and COM exports        |
| `modules/validation`      | Disposable fixtures, path containment, cleanup, and child-process utilities                   |
| `modules/xtask`           | SVG icon generation, versioned packaging, signing pipeline, benchmarks, and JSON reports      |
| `assets`                  | Editable SVG icon source and generated Windows PNG / multi-resolution ICO resources           |
| `packaging`               | Fixed product identities, sparse MSIX template, Inno script, and signing helper               |

The core is independent of the CLI and UI. Callers use `prepare`, display its returned targets, then pass `PreparedTargets` to `execute`. Invalid input prevents the entire batch from starting; execution can partially succeed.

`execute_with_progress` adds an optional observer with snapshots at most ten times per second, outside engine locks. A scoped monitor joins before returning; CLI execution without an observer keeps the existing worker budget. `DiagnosticCode` classifies known domain errors without matching English strings. Unknown OS failures retain their numeric error and system explanation.

Shell copies a complete selection into an ACL-restricted, create-new JSON request before launching the absolute active desktop executable. COM interfaces stay on their owning thread; a DLL reference protects copied-path background dispatch. The DLL links integration and i18n, and delegates all deletion to the desktop process. Installed tasks hold shared `LockFileEx` leases; maintenance takes an exclusive lease and publishes immutable version directories. Owned registry values, PATH deltas, file hashes, and a durable journal bound rollback and uninstall.

## Deletion and concurrency

The thread budget is 1–64 and includes the scanner; the default is the smaller of available CPU threads and 4. One thread runs synchronously. Larger budgets use one scanner and the remaining threads for deletion through a bounded channel. Directories are deleted after their enumerated children finish. Confirmation shows top-level targets without scanning for an exact recursive count.

Cancellation stops operations that have not started; active system calls must return. Success counts reflect completed operations. Dry runs traverse without deleting. Failure details are capped at 100 while the total count remains accurate.

Reparse points are never recursively traversed. Preflight records each top-level target's volume identity, file identity, and entry type; all targets are reopened and checked before execution. Scanned directories and ancestors retain handles that deny write / delete sharing. Child tasks keep required parent handles alive. Internal directory types are revalidated under a read lock; completion closes handles under a write lock.

Leaf handles allow delete sharing for hard links. Deletion stays bound to the opened object even if another process moves it outside the selected directory. A replacement made before opening may be deleted. Attribute-only changes can bypass sharing restrictions, leaving a race between enumeration, validation, and opening. The engine does not isolate malicious concurrent changes.

Deletion uses `SetFileInformationByHandle(FileDispositionInfoEx)` with POSIX semantics, falling back to ordinary handle-based deletion when unsupported. Permission and sharing failures are reported; read-only attributes are preserved. Successful entry deletion does not guarantee immediate handle invalidation or disk-space reclamation. See Microsoft's [deletion API](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle) and [sharing rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).

## Native confirmation

`TaskDialogIndirect` shows a warning, Delete, and Cancel. Cancel is the default; Escape and window close also cancel. Single targets show name and location, multiple targets show the top-level count, and expanded details show full paths. Content sizing and grapheme-based wrapping preserve long paths, combining characters, and joined emoji. Execution uses the original paths. The dialog explains permanent deletion and cancellation limits.

`build.rs` embeds a Common Controls v6 manifest with current-user privileges, long-path support, and DPI awareness. Dialog failure returns a confirmation error; console confirmation requires explicit selection. See Microsoft's [task dialog documentation](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-taskdialogindirect).

The desktop process uses separate confirmation, execution, and result TaskDialogs. Execution displays activity and counts without an exact percentage; Cancel, Escape, and window close request cancellation and wait for active operations. Settings stage language/menu/PATH choices before calling maintenance. All packaging builds embed four-part versions and icon resources. Certificate-backed builds also embed matching sparse-package name, Publisher, and application ID; unsigned builds omit the desktop package identity and MSIX.

`cargo xtask pack` defaults to an installable unsigned package with classic menus on Windows 10 and Windows 11's Show more options. Supplying Publisher, certificate thumbprint, and timestamp URL together enables EXE/DLL, MSIX, and Setup signing and the Windows 11 modern menu. Both modes share fixed product identities, installation paths, and output names. Payload schema 1 records a `signed` boolean: signature checks apply to signed payloads, while file hashes, ACLs, installation leases, ownership checks, and transaction protection apply to both modes.

## Dependencies

Runtime dependencies provide Win32 bindings (`windows-sys` for existing engine code; typed `windows`/matching `windows-core` 0.62.2 for COM and deployment), argument parsing (`clap`), bounded channels (`crossbeam-channel`), cancellation (`ctrlc`), grapheme segmentation (`unicode-segmentation`), localization (`fluent-bundle` 0.16.0 / `unic-langid` 0.9.6), and versioned protocols/manifests (`serde`, `serde_json`, `sha2`). Versions and features are declared in [Cargo.toml](../Cargo.toml) and resolved in [Cargo.lock](../Cargo.lock).

The development-only icon generator uses [resvg](https://github.com/linebender/resvg) 0.48.1 with font and raster-image features disabled to render vector geometry, and [ico](https://github.com/mdsteele/rust-ico) 0.6.0 to encode Windows icons. Neither dependency is linked into the application binaries.

### Icon assets

[`assets/Unlinka.svg`](../assets/Unlinka.svg) is the editable source for the application icon: white geometric open links separated by a diagonal break on a black rounded square, with transparent outer corners. It uses no fonts, gradients, or external images. Keep the canvas square when redrawing it.

```powershell
cargo xtask assets
cargo xtask assets --check
```

The first command writes the committed resources in `assets/windows`; the second compares them with the SVG without changing files and fails on missing or outdated resources. The ICO contains native 16, 20, 24, 32, 40, 48, 64, 96, 128, and 256 px frames. Small frames use BMP encoding; the 256 px frame uses PNG. PNG resources include the base manifest logos, 100–400% scale variants, 14 app-list target sizes with default / light / dark filename qualifiers, and a 256 px preview. All sizes are rendered directly from SVG, and the same black-and-white design is used for both themes. Asset names and sizes follow [Microsoft's icon construction guidance](https://learn.microsoft.com/en-us/windows/apps/design/iconography/app-icon-construction).

`cargo xtask pack` renders its own fresh resources from the SVG into the package output before compiling EXE / DLL icons and the installer. It copies only the generated Windows resources into the installed `Assets` directory, retaining the manifest and Explorer path contracts.

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

[CI](../.github/workflows/ci.yml) runs on pushes, pull requests, and manual dispatch. The Windows job uses stable MSVC, rustfmt, Clippy, and Rusteward for compilation, formatting, lint, and unit tests, checks generated icons against the SVG, builds the default unsigned installer with a signature-checked portable Inno 7.1 compiler, then runs sequential release-mode acceptance and cancellation tests. Acceptance uses one test thread and reports symbolic-link privilege skips; cancellation checks 1 / 8 deletion threads. Cargo commands use `--locked`. Benchmarks run manually. CI does not install the product, register packages/menus, or change certificate trust.

### Markdown maintenance

Local checks and CI share [`.rumdl.toml`](../.rumdl.toml): `.gitignore` is respected, MD013 is disabled, and MD060 requires aligned tables. CI uses rumdl 0.2.79 and fails on configuration warnings.

```powershell
uv tool install rumdl==0.2.79

# Fix only files changed in the task, then review the diff
rumdl check --fix README.md docs/architecture.md

# Check all documentation without rewriting
rumdl check --deny-config-warnings
```
