use std::{
    fmt::Write as _,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use unlinka_validation::{Fixture, run_command};

#[cfg(test)]
mod tests;

const METHODS: [&str; 5] = [
    "unlinka-1",
    "unlinka-4",
    "unlinka-8",
    "dotnet",
    "powershell",
];
const FILE_BYTES: usize = 1024;
const METHODOLOGY: &str = "Fresh 1 KiB plain files; flat has 1 directory, tree has 111. Warm metadata after creation/count verification; creation excluded; rotated method order. Unlinka operation time is from its execute report (1 ms precision); wall includes startup, preflight and reporting. .NET Directory.Delete and PowerShell Remove-Item operation timers run inside fresh pwsh children; their wall times include startup. No cache flush, background security changes or symlinks. Each child is limited to 120 seconds. Disposable data is removed on exit; JSON evidence is retained.";

// Paths travel through an environment variable, never through script interpolation.
const BASELINE: &str = r"
$ErrorActionPreference = 'Stop'
$watch = [Diagnostics.Stopwatch]::StartNew()
if ($env:UNLINKA_BENCH_METHOD -eq 'dotnet') {
    [IO.Directory]::Delete($env:UNLINKA_BENCH_TARGET, $true)
} else {
    Remove-Item -LiteralPath $env:UNLINKA_BENCH_TARGET -Recurse -Force
}
$watch.Stop()
$watch.Elapsed.TotalMilliseconds.ToString('R', [Globalization.CultureInfo]::InvariantCulture)
";

const ENVIRONMENT: &str = r"
$ErrorActionPreference = 'Stop'
$os = Get-CimInstance Win32_OperatingSystem
$storage = @{ fileSystem = $null; disk = $null; bus = $null; storageError = $null }
try {
    $root = [IO.Path]::GetPathRoot($env:UNLINKA_BENCH_TARGET)
    if ($root.StartsWith('\\?\')) { $root = $root.Substring(4) }
    if ($root -notmatch '^[A-Za-z]:\\$') { throw 'Storage details require a local drive-letter volume' }
    $drive = $root.Substring(0,1)
    $volume = Get-Volume -DriveLetter $drive
    $disk = Get-Partition -DriveLetter $drive | Get-Disk
    $storage.fileSystem = $volume.FileSystem
    $storage.disk = $disk.FriendlyName
    $storage.bus = [string]$disk.BusType
} catch { $storage.storageError = $_.Exception.Message }
@{ createdUtc = [DateTime]::UtcNow.ToString('o'); os = $os.Caption; build = $os.BuildNumber;
   fileSystem = $storage.fileSystem; disk = $storage.disk; bus = $storage.bus; storageError = $storage.storageError;
   logicalProcessors = [Environment]::ProcessorCount; powershell = [string]$PSVersionTable.PSVersion;
   dotnet = [string][Environment]::Version } | ConvertTo-Json -Compress
";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    shape: String,
    round: u32,
    method: String,
    files: u32,
    file_bytes: usize,
    directories: u64,
    operation_milliseconds: f64,
    wall_milliseconds: f64,
    exit_code: i32,
    target_absent: bool,
    sentinel_unchanged: bool,
    stderr: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    shape: String,
    method: String,
    rounds: usize,
    median_operation_milliseconds: f64,
    median_wall_milliseconds: f64,
    files_per_second: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Results {
    created_utc: Value,
    root: PathBuf,
    source_binary: PathBuf,
    binary_sha256: String,
    environment: Value,
    methodology: &'static str,
    status: &'static str,
    error: Option<String>,
    summary: Vec<Summary>,
    samples: Vec<Sample>,
}

pub fn run(
    binary: Option<PathBuf>,
    file_count: u32,
    rounds: u32,
    shapes: &[String],
) -> io::Result<()> {
    if shapes
        .iter()
        .enumerate()
        .any(|(index, shape)| shapes[..index].contains(shape))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "benchmark shapes must not be repeated",
        ));
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let source_binary = resolve_binary(&workspace, binary)?;
    let fixture = Fixture::new("benchmark")?;
    let binary = fixture.path("unlinka.exe")?;
    fs::copy(&source_binary, &binary)?;
    let sentinel = fixture.path("outside-target.txt")?;
    fs::write(&sentinel, b"benchmark sentinel")?;
    let sentinel_hash = hash(&sentinel)?;
    let environment = read_environment(fixture.root())?;
    let mut result = Results {
        created_utc: environment["createdUtc"].clone(),
        root: fixture.root().to_owned(),
        source_binary,
        binary_sha256: hash(&binary)?,
        environment,
        methodology: METHODOLOGY,
        status: "running",
        error: None,
        summary: Vec::new(),
        samples: Vec::new(),
    };
    // A separate, fresh evidence file survives the fixture's automatic cleanup.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let evidence = fixture
        .root()
        .parent()
        .ok_or_else(|| io::Error::other("fixture has no parent"))?
        .join(format!(
            "benchmark-results-{}-{stamp}.json",
            std::process::id()
        ));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&evidence)?;
    save(&evidence, &result)?;
    println!("Benchmark data: {}", fixture.root().display());
    println!("Results: {}", evidence.display());
    let context = SampleContext {
        fixture: &fixture,
        binary: &binary,
        sentinel: &sentinel,
        sentinel_hash: &sentinel_hash,
        file_count,
    };
    let execution: io::Result<()> = (|| {
        for shape in shapes {
            for round in 1..=rounds {
                // Rotate the first method to reduce systematic ordering effects.
                for offset in 0..METHODS.len() {
                    let method = METHODS[(offset + round as usize - 1) % METHODS.len()];
                    let sample = sample(&context, method, round, shape)?;
                    println!(
                        "{shape} round {round} {method}: operation {:.3} ms, wall {:.3} ms",
                        sample.operation_milliseconds, sample.wall_milliseconds
                    );
                    result.samples.push(sample);
                    save(&evidence, &result)?;
                }
            }
        }
        Ok(())
    })();
    result.status = if execution.is_ok() {
        "passed"
    } else {
        "failed"
    };
    if let Err(ref error) = execution {
        result.error = Some(error.to_string());
    }
    result.summary = summarize(&result.samples, shapes, file_count);
    save(&evidence, &result)?;
    for row in &result.summary {
        println!(
            "{} {}: median operation {:.3} ms, wall {:.3} ms ({:.0} files/s)",
            row.shape,
            row.method,
            row.median_operation_milliseconds,
            row.median_wall_milliseconds,
            row.files_per_second
        );
    }
    execution
}

fn read_environment(root: &Path) -> io::Result<Value> {
    let output = powershell(ENVIRONMENT, root, "environment", root)?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        io::Error::other(format!(
            "cannot parse PowerShell environment JSON as UTF-8: {error}"
        ))
    })
}

fn resolve_binary(workspace: &Path, supplied: Option<PathBuf>) -> io::Result<PathBuf> {
    if let Some(binary) = supplied {
        return binary.canonicalize();
    }
    let output = Command::new("cargo")
        .args([
            "build",
            "--package",
            "unlinka",
            "--bin",
            "unlinka",
            "--release",
            "--locked",
            "--message-format=json",
        ])
        .current_dir(workspace)
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("release build failed"));
    }
    // Cargo's artifact path also honors configured target triples/directories.
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: Value = serde_json::from_slice(line).map_err(|error| {
            io::Error::other(format!("cannot parse Cargo build artifact JSON: {error}"))
        })?;
        if message["reason"] == "compiler-artifact" && message["target"]["name"] == "unlinka" {
            if let Some(executable) = message["executable"].as_str() {
                return Path::new(executable).canonicalize();
            }
        }
    }
    Err(io::Error::other(
        "Cargo did not emit the unlinka executable path",
    ))
}

fn powershell(
    script: &str,
    target: &Path,
    method: &str,
    cwd: &Path,
) -> io::Result<std::process::Output> {
    let mut command = Command::new("pwsh");
    // Redirected pwsh can inherit a legacy Windows code page (for example CP936).
    // Set its pipe encoding explicitly so JSON preserves localized OS/disk names.
    let script = format!("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n{script}");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .env("UNLINKA_BENCH_TARGET", target)
        .env("UNLINKA_BENCH_METHOD", method)
        .current_dir(cwd);
    let output = run_command(&mut command).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("PowerShell 7 (pwsh) baseline: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{method} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

struct SampleContext<'a> {
    fixture: &'a Fixture,
    binary: &'a Path,
    sentinel: &'a Path,
    sentinel_hash: &'a str,
    file_count: u32,
}

/// Generate and verify a fresh dataset before the timed deletion phase.
fn prepare_sample(
    fixture: &Fixture,
    name: &str,
    shape: &str,
    file_count: u32,
) -> io::Result<(PathBuf, u64)> {
    let target = fixture.path(name)?;
    fs::create_dir(&target)?;
    let mut folders = Vec::new();
    let directories = if shape == "flat" {
        folders.push(target.clone());
        1
    } else {
        for group in 0..10 {
            for bucket in 0..10 {
                folders.push(fixture.folder(format!("{name}/group-{group}/bucket-{bucket}"))?);
            }
        }
        111
    };
    let payload: Vec<u8> = (0..FILE_BYTES)
        .map(|index| u8::try_from(index % 251).expect("remainder fits in u8"))
        .collect();
    for index in 0..file_count {
        let path = folders[index as usize % folders.len()].join(format!("file-{index:06}.bin"));
        fs::write(
            fixture.path(
                path.strip_prefix(fixture.root())
                    .map_err(io::Error::other)?,
            )?,
            &payload,
        )?;
    }
    if count_entries(&target)? != (u64::from(file_count), directories) {
        return Err(io::Error::other("incomplete benchmark fixture"));
    }
    // Revalidate containment immediately before passing the destructive target.
    Ok((fixture.path(name)?, directories))
}

fn sample(
    context: &SampleContext<'_>,
    method: &str,
    round: u32,
    shape: &str,
) -> io::Result<Sample> {
    let SampleContext {
        fixture,
        binary,
        sentinel,
        sentinel_hash,
        file_count,
    } = context;
    let file_count = *file_count;
    let name = format!("{shape}-{round}-{method}");
    let (target, directories) = prepare_sample(fixture, &name, shape, file_count)?;
    let started = Instant::now();
    let output = if let Some(threads) = method.strip_prefix("unlinka-") {
        let mut command = Command::new(binary);
        command
            .args(["-r", "--yes", "--json", "--threads", threads, "--"])
            .arg(&target)
            .current_dir(fixture.root());
        run_command(&mut command)?
    } else {
        powershell(BASELINE, &target, method, fixture.root())?
    };
    let wall_milliseconds = started.elapsed().as_secs_f64() * 1000.0;
    let stderr = String::from_utf8(output.stderr).map_err(io::Error::other)?;
    if !output.status.success() {
        return Err(io::Error::other(format!("{method} failed: {stderr}")));
    }
    let stdout = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let operation_milliseconds = if method.starts_with("unlinka-") {
        let report: Value = serde_json::from_str(&stdout).map_err(io::Error::other)?;
        if report["schema_version"] != 1
            || report["files"].as_u64() != Some(u64::from(file_count))
            || report["directories"].as_u64() != Some(directories)
            || report["links"] != 0
            || report["failed"] != 0
            || report["cancelled"] != false
            || report["exit_code"] != 0
        {
            return Err(io::Error::other(format!(
                "incorrect deletion report: {stderr}"
            )));
        }
        report["elapsed_seconds"]
            .as_f64()
            .ok_or_else(|| io::Error::other("deletion elapsed time missing"))?
            * 1000.0
    } else {
        stdout.trim().parse::<f64>().map_err(io::Error::other)?
    };
    if !operation_milliseconds.is_finite() || operation_milliseconds < 0.0 {
        return Err(io::Error::other("invalid operation time"));
    }
    let target_absent = match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
        Ok(_) => false,
    };
    let sentinel_unchanged = hash(sentinel)? == *sentinel_hash;
    if !target_absent || !sentinel_unchanged {
        return Err(io::Error::other("deletion or sentinel verification failed"));
    }
    Ok(Sample {
        shape: shape.to_owned(),
        round,
        method: method.to_owned(),
        files: file_count,
        file_bytes: FILE_BYTES,
        directories,
        operation_milliseconds,
        wall_milliseconds,
        exit_code: output.status.code().unwrap_or(-1),
        target_absent,
        sentinel_unchanged,
        stderr,
    })
}

fn count_entries(target: &Path) -> io::Result<(u64, u64)> {
    let mut files = 0;
    let mut directories = 1;
    for entry in fs::read_dir(target)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let child = count_entries(&entry.path())?;
            files += child.0;
            directories += child.1;
        } else if kind.is_file() {
            files += 1;
        } else {
            return Err(io::Error::other(
                "benchmark fixture contains an unexpected entry",
            ));
        }
    }
    Ok((files, directories))
}

fn hash(path: &Path) -> io::Result<String> {
    let digest = Sha256::digest(fs::read(path)?);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(hex)
}

fn save(path: &Path, result: &Results) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    serde_json::to_writer_pretty(&mut file, result).map_err(io::Error::other)?;
    file.write_all(b"\n")
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        f64::midpoint(values[middle - 1], values[middle])
    }
}

fn summarize(samples: &[Sample], shapes: &[String], file_count: u32) -> Vec<Summary> {
    let mut summary = Vec::new();
    for shape in shapes {
        for method in METHODS {
            let selected: Vec<_> = samples
                .iter()
                .filter(|sample| sample.shape == *shape && sample.method == method)
                .collect();
            if selected.is_empty() {
                continue;
            }
            let operation = median(
                selected
                    .iter()
                    .map(|sample| sample.operation_milliseconds)
                    .collect(),
            );
            summary.push(Summary {
                shape: shape.clone(),
                method: method.to_owned(),
                rounds: selected.len(),
                median_operation_milliseconds: operation,
                median_wall_milliseconds: median(
                    selected
                        .iter()
                        .map(|sample| sample.wall_milliseconds)
                        .collect(),
                ),
                files_per_second: if operation > 0.0 {
                    f64::from(file_count) * 1000.0 / operation
                } else {
                    0.0
                },
            });
        }
    }
    summary
}
