# Architecture

Unlinka is a Windows-only Rust workspace with a CLI, an independent deletion engine, native task dialogs, a classic Explorer COM adapter, and a current-user installer.

[Windows integration and i18n](windows-integration.md) defines the implemented installation layout, registration contracts, lifecycle recovery, and packaging. Release signing and real Windows menu/lifecycle acceptance remain pending.

## Responsibilities

| Package / module          | Responsibility                                                                             |
| ------------------------- | ------------------------------------------------------------------------------------------ |
| `modules/app` / `cli`     | Arguments, thread budget, and exit status                                                  |
| `modules/app` / `main`    | Preflight → confirmation → execution → summary, plus cancellation                          |
| `modules/app` / `confirm` | Interactive terminal confirmation                                                          |
| `modules/app` / `result`  | Concise terminal reports, versioned JSON results, stable diagnostics, and raw UTF-16 paths |
| `modules/core` / `paths`  | Path resolution, input protection, target identity, and duplicate / overlap normalization  |
| `modules/core` / `engine` | Bounded scheduling, scanning, deletion, directory completion, and summaries                |
| `modules/core` / `win`    | UTF-16, handles, reparse-point handling, enumeration, and deletion APIs                    |
| `modules/i18n`            | Embedded Fluent resources, locale negotiation, shared path/error/outcome/count formatting  |
| `modules/integration`     | Installation leases, requests, registry/PATH/file ownership, transactions, and maintenance |
| `modules/launcher`        | Stable installed CLI entry, lossless argument forwarding, and child exit/cancellation      |
| `modules/desktop`         | Task wording, deletion workers, cancellation, results, and staged settings                 |
| `modules/ui`              | WinSafe task dialogs, modal details viewer, progress events, and basic diagnostics         |
| `modules/shell`           | Classic `IContextMenu`, selection adapters, and COM exports                                |
| `modules/validation`      | Disposable fixtures, path containment, cleanup, and child-process utilities                |
| `modules/xtask`           | SVG icon generation, versioned packaging, signing pipeline, benchmarks, and JSON reports   |
| `assets`                  | Editable SVG icon source and generated Windows PNG / multi-resolution ICO resources        |
| `packaging`               | Fixed product identifiers, Inno script, and signing helper                                 |

The core is independent of the CLI and UI. Callers use `prepare`, display its returned targets, then pass `PreparedTargets` to `execute`. Invalid input prevents the entire batch from starting; execution can partially succeed.

`execute_with_progress` adds an optional observer with snapshots at most ten times per second, outside engine locks. A scoped monitor joins before returning; CLI execution without an observer keeps the existing worker budget. `DiagnosticCode` classifies known domain errors without matching English strings. Unknown OS failures retain their numeric error and system explanation.

Shell copies a complete selection into an ACL-restricted, create-new JSON request before launching the absolute active desktop executable. COM interfaces stay on their owning thread; a DLL reference protects copied-path background dispatch. The DLL links integration and i18n, and delegates all deletion to the desktop process. Installed tasks hold shared `LockFileEx` leases; maintenance takes an exclusive lease and publishes immutable version directories. Owned registry values, PATH deltas, file hashes, and a durable journal bound rollback and uninstall.

## Deletion and concurrency

The thread budget is 1–64 and includes the scanner; the default is the smaller of available CPU threads and 4. One thread runs synchronously. Larger budgets use one scanner and the remaining threads for deletion through a bounded channel. Directories are deleted after their enumerated children finish. Confirmation shows top-level targets without scanning for an exact recursive count.

Cancellation stops operations that have not started; active system calls must return. Success counts reflect completed operations. Dry runs traverse without deleting. Failure details are capped at 100 while the total count remains accurate.

Reparse points are never recursively traversed. Preflight records each top-level target's volume identity, file identity, and entry type; all targets are reopened and checked before execution. Scanned directories and ancestors retain handles that deny write / delete sharing. Child tasks keep required parent handles alive. Internal directory types are revalidated under a read lock; completion closes handles under a write lock.

Leaf handles allow delete sharing for hard links. Deletion stays bound to the opened object even if another process moves it outside the selected directory. A replacement made before opening may be deleted. Attribute-only changes can bypass sharing restrictions, leaving a race between enumeration, validation, and opening. The engine does not isolate malicious concurrent changes.

Deletion uses `SetFileInformationByHandle(FileDispositionInfoEx)` with POSIX semantics, falling back to ordinary handle-based deletion when unsupported. Permission and sharing failures are reported; read-only attributes are preserved. Successful entry deletion does not guarantee immediate handle invalidation or disk-space reclamation. See Microsoft's [deletion API](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle) and [sharing rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew).

## Confirmation and native Shell tasks

CLI deletion confirms in the terminal using interactive stdin and stderr. The prompt lists all prepared top-level targets, flushes stderr before reading, and accepts only `y` or `yes` (case-insensitive, with surrounding whitespace ignored). Empty or other answers cancel; EOF and I/O failures return confirmation error status 4. `--yes` / `-y` bypasses confirmation, and `--dry-run` scans without confirming or deleting. Native UI is confined to the desktop/Shell path; the CLI has no confirmation-interface selector or task-dialog dependency.

`TaskDialogIndirect` asks for permanent deletion with View targets, Delete permanently, and Cancel. Cancel is the default; Escape and window close also cancel. Single targets show name and location, while multiple targets show the top-level count. Main summaries shorten long names and paths at grapheme boundaries. Complete paths appear in an owned, fixed-size WinSafe `WindowMain` with `Edit` and `Button` controls. The read-only multiline edit supports wrapping, vertical scrolling, selection, and copying. The viewer initially scales its bounds, controls, and font to the owner's DPI and updates them on `WM_DPICHANGED`. It disables the owner for its nested message loop, then restores it and consumes its own quit message. Viewing or closing details never confirms deletion or resizes the main dialog. Execution uses the original paths. See Microsoft's [owned window documentation](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features#owned-windows) and [edit control styles](https://learn.microsoft.com/en-us/windows/win32/controls/edit-control-styles).

`unlinka-ui` owns native controls, strings crossing Windows calls, callback error/panic handling, and the small set of task-dialog messages and DPI operations missing from WinSafe. It has no dependency on the deletion engine, installation state, or localization. Desktop and maintenance supply wording and action IDs. The `dialogs` feature enables task dialogs and the viewer; Shell uses only the basic diagnostic API. The integration library's optional `maintenance-ui` feature is enabled by default for its maintenance executable and disabled by workspace library consumers, so an isolated CLI or Shell build does not acquire maintenance dialogs.

All four executables embed the shared [Windows manifest](../packaging/unlinka.manifest) through [one build helper](../packaging/windows.rs), including Common Controls v6, current-user privileges, long-path support, and DPI awareness. Common Controls v6 must be selected before the loader resolves `TaskDialogIndirect`; initializing controls in `main` cannot repair a missing import. Shell dialog failure returns a confirmation error. See Microsoft's [task dialog documentation](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-taskdialogindirect).

The desktop process uses separate confirmation, execution, and result TaskDialogs. Its task controller starts the deletion worker only after the progress dialog reports successful creation, observes progress, and owns cancellation and joining. Dialog creation failure cannot start deletion; callback errors or panics cancel and join an existing worker before returning. Execution displays activity and counts without an exact percentage; Cancel, Escape, and window close request cancellation and wait for active operations. Cancellation changes the heading while preserving counts. Results distinguish complete, incomplete, and cancelled execution; failures keep their total and first path/reason visible, with up to 100 retained details in the same scrollable viewer. CLI reports show up to 5 failures by default; `-v` exposes all retained details, operations, and codes. Common Windows permission, sharing, missing-item, write-protection, and nonempty-directory errors are localized without hiding unknown system descriptions. JSON and engine failure retention remain unchanged.

Settings stage language/menu/PATH choices before calling maintenance. All packaging builds embed four-part versions and icon resources. Optional Authenticode signing covers the EXE/DLL payload, Setup, and its uninstaller.

`cargo xtask pack` defaults to an installable unsigned package with classic menus on Windows 10 and Windows 11's Show more options. Supplying certificate Subject through `--publisher`, certificate thumbprint, and timestamp URL together enables EXE/DLL and Setup signing. Signing does not change the menu registration. Both modes share fixed product identities, installation paths, and output names. Payload schema 1 records a `signed` boolean and `signer_subject`: signature checks apply to signed payloads, while file hashes, ACLs, installation leases, ownership checks, and transaction protection apply to both modes.

## Dependencies

Runtime dependencies provide native UI ([WinSafe](https://github.com/rodrigocfd/winsafe) 0.0.29), Win32 bindings (`windows-sys` for existing engine code and private UI message/DPI helpers; typed `windows`/matching `windows-core` 0.62.2 for COM and Windows integration), argument parsing (`clap`), bounded channels (`crossbeam-channel`), cancellation (`ctrlc`), grapheme segmentation (`unicode-segmentation`), localization (`fluent-bundle` 0.16.0 / `unic-langid` 0.9.6), and versioned protocols/manifests (`serde`, `serde_json`, `sha2`). Versions and features are declared in [Cargo.toml](../Cargo.toml) and resolved in [Cargo.lock](../Cargo.lock).

The development-only compiler acquisition uses `ureq` for HTTPS and `zip` for archive extraction. An explicit `flate2` Rust backend keeps extraction from changing the renderer's PNG compression through Cargo feature unification. It uses a fixed compiler archive SHA-256 and checks cached files against that verified archive before invoking the compiler. The archive source, version, and cache behavior are described in [Windows integration](windows-integration.md#61-构建统一安装包).

The development-only icon generator uses [resvg](https://github.com/linebender/resvg) 0.48.1 with font and raster-image features disabled to render vector geometry, and [ico](https://github.com/mdsteele/rust-ico) 0.6.0 to encode Windows icons. Neither dependency is linked into the application binaries.

### Icon assets

[`assets/Unlinka.svg`](../assets/Unlinka.svg) is the editable source for the application icon: white geometric open links separated by a diagonal break on a black rounded square, with transparent outer corners. It uses no fonts, gradients, or external images. Keep the canvas square when redrawing it.

```powershell
cargo xtask assets
cargo xtask assets --check
```

The first command writes the committed resources in `assets/windows`; the second compares them with the SVG without changing files and fails on missing or outdated resources. The ICO contains native 16, 20, 24, 32, 40, 48, 64, 96, 128, and 256 px frames. Small frames use BMP encoding; the 256 px frame uses PNG. A 256 px PNG provides the preview. All sizes are rendered directly from SVG, and the same black-and-white design is used for both themes.

`cargo xtask pack` renders its own fresh resources from the SVG into the package output before compiling EXE / DLL icons and the installer. Resource scripts use UTF-8 explicitly so non-ASCII checkout paths remain valid. Packaging enables the static MSVC C runtime for all distributed EXE/DLL files, including when inherited compiler flags request dynamic CRT, so the installer does not require a separate VC++ Redistributable. The generated images are build resources; installed applications use their embedded icons.

## Development and validation

Builds target Windows 10/11 with MSVC. The declared minimum Rust version, local compiler, and CI toolchain are Rust 1.99. Validation results and remaining gaps are in [acceptance status](acceptance.md).

```powershell
cargo check --workspace --all-targets --locked
cargo dev format --check --diff --locked
cargo dev lint --locked
cargo test --workspace --lib --bins --locked
cargo build --release --locked
```

`cargo dev` uses [Rusteward](https://github.com/RhNu/Rusteward). The test command runs unit tests; ordinary `cargo test --workspace` skips opt-in deletion checks. Integration and benchmark commands and manual UI checks are documented in [acceptance status](acceptance.md). [`.gitattributes`](../.gitattributes) keeps Rust sources in LF form.

### Continuous integration

[CI](../.github/workflows/ci.yml) runs on pushes, pull requests, and manual dispatch. The Windows job uses Rust 1.99 MSVC, rustfmt, Clippy, and Rusteward for compilation, formatting, lint, and unit tests, checks generated icons against the SVG, then runs sequential release-mode acceptance and cancellation tests. Acceptance uses one test thread and reports symbolic-link privilege skips; cancellation checks 1 / 8 deletion threads. Cargo commands use `--locked`. Installer packaging and benchmarks run manually. CI does not install the product, register menus, or change certificate trust.

### Markdown maintenance

Local checks and CI share [`.rumdl.toml`](../.rumdl.toml): `.gitignore` is respected, MD013 is disabled, and MD060 requires aligned tables. CI uses rumdl 0.2.79 and fails on configuration warnings.

```powershell
uv tool install rumdl==0.2.79

# Fix only files changed in the task, then review the diff
rumdl check --fix README.md docs/architecture.md

# Check all documentation without rewriting
rumdl check --deny-config-warnings
```
