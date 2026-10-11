#![cfg(windows)]

#[path = "acceptance/support.rs"]
mod support;

use std::{
    ffi::OsString,
    fs,
    os::windows::fs::{OpenOptionsExt, symlink_dir, symlink_file},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use unlinka_core::{DeleteOptions, DeleteReport, execute, prepare};
use unlinka_validation::{Counts, Fixture, counts, run};

fn file(path: &Path) { fs::write(path, b"acceptance sentinel").expect("write fixture file"); }

fn basic(fixture: &Fixture, name: &str) -> PathBuf {
    let target = fixture.folder(name).expect("create fixture target");
    let sub = fixture
        .folder(format!("{name}/sub"))
        .expect("create fixture subtree");
    fixture
        .folder(format!("{name}/empty"))
        .expect("create empty fixture directory");
    file(&target.join("first.txt"));
    file(&sub.join("second.txt"));
    file(&sub.join("third.txt"));
    target
}

fn engine(targets: &[PathBuf], threads: usize, dry_run: bool) -> DeleteReport {
    execute(
        prepare(targets, true).expect("prepare disposable targets"),
        &DeleteOptions {
            recursive: true,
            threads,
            dry_run,
        },
        &AtomicBool::new(false),
    )
}

fn assert_report(report: &DeleteReport, files: u64, directories: u64, links: u64, failed: u64) {
    assert_eq!(
        (
            report.files,
            report.directories,
            report.links,
            report.failed
        ),
        (files, directories, links, failed),
        "{report:?}"
    );
    assert!(!report.cancelled);
}

fn cli(fixture: &Fixture, options: &[&str], targets: &[PathBuf], cwd: &Path, exit: i32) -> String {
    let copy = fixture
        .path("unlinka.exe")
        .expect("validate executable copy path");
    if !copy.exists() {
        fs::copy(env!("CARGO_BIN_EXE_unlinka"), &copy).expect("copy executable inside fixture");
    }
    let mut args: Vec<OsString> = options.iter().map(OsString::from).collect();
    args.push("--".into());
    args.extend(targets.iter().map(|path| path.as_os_str().to_owned()));
    let output = run(&copy, &args, cwd).expect("run validation executable within timeout");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(exit), "{text}");
    text
}

#[test]
#[ignore = "creates and deletes disposable Windows filesystem fixtures"]
fn recursive_dry_run_and_cli_counts() {
    let fixture = Fixture::new("acceptance-basic").unwrap();
    assert!(fixture.path("../escape.txt").is_err());
    assert!(fixture.path(fixture.root()).is_err());
    for threads in [1, 8] {
        let target = basic(&fixture, &format!("basic-{threads}"));
        let before = support::snapshot(&target).unwrap();
        assert_report(
            &engine(std::slice::from_ref(&target), threads, true),
            3,
            3,
            0,
            0,
        );
        let text = cli(
            &fixture,
            &["-r", "--dry-run", "--threads", &threads.to_string()],
            std::slice::from_ref(&target),
            fixture.root(),
            0,
        );
        assert_eq!(
            counts(&text).unwrap(),
            Counts {
                files: 3,
                directories: 3,
                links: 0,
                failed: 0
            }
        );
        assert_eq!(before, support::snapshot(&target).unwrap());
        let text = cli(
            &fixture,
            &["-r", "-y", "--threads", &threads.to_string()],
            std::slice::from_ref(&target),
            fixture.root(),
            0,
        );
        assert_eq!(
            counts(&text).unwrap(),
            Counts {
                files: 3,
                directories: 3,
                links: 0,
                failed: 0
            }
        );
        assert!(!target.exists());
        let core_target = basic(&fixture, &format!("core-{threads}"));
        assert_report(
            &engine(std::slice::from_ref(&core_target), threads, false),
            3,
            3,
            0,
            0,
        );
        assert!(!core_target.exists());
    }
}

#[test]
#[ignore = "creates and deletes disposable Windows filesystem fixtures"]
fn unicode_spaces_and_long_paths() {
    let fixture = Fixture::new("acceptance-paths").unwrap();
    let target = fixture.folder("路径 with spaces").unwrap();
    file(&target.join("漢字 😊 spaces.txt"));
    let deep = fixture
        .folder(format!(
            "路径 with spaces/{}/{}/{}",
            "a".repeat(85),
            "b".repeat(85),
            "c".repeat(85)
        ))
        .unwrap();
    file(&deep.join("long 漢字.txt"));
    assert_report(&engine(std::slice::from_ref(&target), 8, false), 2, 4, 0, 0);
    assert!(!target.exists());
}

#[test]
#[ignore = "creates and deletes Windows junctions and disposable fixtures"]
fn junction_targets_are_not_traversed() {
    let fixture = Fixture::new("acceptance-junctions").unwrap();
    let external = fixture.folder("external").unwrap();
    file(&external.join("retain.txt"));
    for internal in [false, true] {
        let container = fixture
            .folder(if internal { "internal" } else { "top" })
            .unwrap();
        let link = container.join("junction");
        support::junction(&link, &external).unwrap();
        let label = if internal { "internal" } else { "top" };
        assert!(
            fixture
                .path(format!("{label}/junction/retain.txt"))
                .is_err()
        );
        assert!(fixture.folder(format!("{label}/junction")).is_err());
        let targets = vec![if internal {
            container.clone()
        } else {
            link.clone()
        }];
        let before = support::snapshot(&container).unwrap();
        assert_report(&engine(&targets, 8, true), 0, u64::from(internal), 1, 0);
        assert_eq!(before, support::snapshot(&container).unwrap());
        let options: &[&str] = if internal { &["-r", "-y"] } else { &["-y"] };
        let text = cli(&fixture, options, &targets, fixture.root(), 0);
        assert_eq!(
            counts(&text).unwrap(),
            Counts {
                files: 0,
                directories: u64::from(internal),
                links: 1,
                failed: 0
            }
        );
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(
            fs::read(external.join("retain.txt")).unwrap(),
            b"acceptance sentinel"
        );
    }
    let disposable = Fixture::new("acceptance-cleanup").unwrap();
    let disposable_root = disposable.root().to_owned();
    let readonly = disposable.path("readonly.txt").unwrap();
    file(&readonly);
    support::readonly(&readonly);
    support::junction(&disposable.path("junction").unwrap(), &external).unwrap();
    drop(disposable);
    assert!(!disposable_root.exists());
    assert_eq!(
        fs::read(external.join("retain.txt")).unwrap(),
        b"acceptance sentinel"
    );
}

#[test]
#[ignore = "requires Windows symbolic-link capability and disposable fixtures"]
fn symlink_targets_are_not_traversed() {
    let fixture = Fixture::new("acceptance-symlinks").unwrap();
    let external = fixture.folder("external").unwrap();
    let sentinel = external.join("retain.txt");
    file(&sentinel);
    for internal in [false, true] {
        let container = fixture
            .folder(if internal { "internal" } else { "top" })
            .unwrap();
        let directory = container.join("directory-link");
        let file_link = container.join("file-link");
        let creation =
            symlink_dir(&external, &directory).and_then(|()| symlink_file(&sentinel, &file_link));
        if let Err(error) = creation {
            if error.raw_os_error() == Some(1314) {
                eprintln!(
                    "SKIPPED symlink-{}: Windows symbolic-link privilege unavailable: {error}",
                    if internal { "internal" } else { "top" }
                );
                continue;
            }
            panic!("symbolic-link creation failed: {error}");
        }
        let targets = if internal {
            vec![container]
        } else {
            vec![directory.clone(), file_link.clone()]
        };
        assert_report(&engine(&targets, 8, false), 0, u64::from(internal), 2, 0);
        assert!(fs::symlink_metadata(&directory).is_err());
        assert!(fs::symlink_metadata(&file_link).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"acceptance sentinel");
    }
}

#[test]
#[ignore = "creates and deletes Windows hard links and disposable fixtures"]
fn hard_links_remove_only_selected_names() {
    let fixture = Fixture::new("acceptance-hardlinks").unwrap();
    let sentinel = fixture.path("retain.txt").unwrap();
    file(&sentinel);
    for internal in [false, true] {
        let container = fixture
            .folder(if internal { "internal" } else { "top" })
            .unwrap();
        let links: Vec<_> = (1..=3)
            .map(|i| container.join(format!("hardlink-{i}.txt")))
            .collect();
        for link in &links {
            fs::hard_link(&sentinel, link).unwrap();
        }
        let targets = if internal {
            vec![container]
        } else {
            links.clone()
        };
        assert_report(&engine(&targets, 8, false), 3, u64::from(internal), 0, 0);
        assert!(links.iter().all(|link| !link.exists()));
        assert_eq!(fs::read(&sentinel).unwrap(), b"acceptance sentinel");
    }
}

#[test]
#[ignore = "creates read-only/locked disposable files and tests partial deletion"]
fn readonly_and_locked_files_produce_partial_failures() {
    let fixture = Fixture::new("acceptance-partial").unwrap();
    let target = fixture.folder("readonly").unwrap();
    let readonly = target.join("readonly.txt");
    file(&readonly);
    file(&target.join("normal.txt"));
    support::readonly(&readonly);
    let before = support::snapshot(&target).unwrap();
    assert_report(&engine(std::slice::from_ref(&target), 8, true), 2, 1, 0, 0);
    assert_eq!(before, support::snapshot(&target).unwrap());
    let text = cli(
        &fixture,
        &["-r", "-y", "--threads", "8"],
        std::slice::from_ref(&target),
        fixture.root(),
        1,
    );
    assert_eq!(
        counts(&text).unwrap(),
        Counts {
            files: 1,
            directories: 0,
            links: 0,
            failed: 2
        }
    );
    assert!(fs::metadata(&readonly).unwrap().permissions().readonly());
    assert!(!target.join("normal.txt").exists());
    let target = fixture.folder("locked").unwrap();
    let held = target.join("held.txt");
    file(&held);
    file(&target.join("normal.txt"));
    let stream = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&held)
        .unwrap();
    let text = cli(
        &fixture,
        &["-r", "-y", "--threads", "8"],
        std::slice::from_ref(&target),
        fixture.root(),
        1,
    );
    assert_eq!(
        counts(&text).unwrap(),
        Counts {
            files: 1,
            directories: 0,
            links: 0,
            failed: 2
        }
    );
    drop(stream);
    assert!(held.exists());
    assert!(!target.join("normal.txt").exists());
}

#[test]
#[ignore = "invokes CLI on disposable input and protected current directories"]
fn validation_and_normalization_precede_mutation() {
    let fixture = Fixture::new("acceptance-validation").unwrap();
    let target = basic(&fixture, "non-recursive");
    let before = support::snapshot(&target).unwrap();
    assert!(prepare(std::slice::from_ref(&target), false).is_err());
    cli(
        &fixture,
        &["-y"],
        std::slice::from_ref(&target),
        fixture.root(),
        2,
    );
    assert_eq!(before, support::snapshot(&target).unwrap());
    let target = basic(&fixture, "overlap");
    let targets = vec![
        target.clone(),
        target.clone(),
        target.join("sub"),
        target.join("sub/second.txt"),
    ];
    assert_eq!(prepare(&targets, true).unwrap().paths().len(), 1);
    let text = cli(
        &fixture,
        &["-r", "-y", "-v", "--threads", "8"],
        &targets,
        fixture.root(),
        0,
    );
    assert!(text.contains("Prepared 1 top-level"));
    assert_eq!(
        counts(&text).unwrap(),
        Counts {
            files: 3,
            directories: 3,
            links: 0,
            failed: 0
        }
    );
    assert!(!target.exists());
    let valid = fixture.path("valid.txt").unwrap();
    file(&valid);
    let targets = vec![valid.clone(), fixture.path("missing.txt").unwrap()];
    assert!(prepare(&targets, true).is_err());
    cli(&fixture, &["-y"], &targets, fixture.root(), 2);
    assert!(valid.exists());
    let parent = fixture.folder("protected").unwrap();
    let cwd = fixture.folder("protected/cwd").unwrap();
    file(&cwd.join("retain.txt"));
    for target in [cwd.clone(), parent] {
        cli(&fixture, &["-r", "-y"], &[target], &cwd, 2);
        assert!(cwd.join("retain.txt").exists());
    }
}

#[test]
#[ignore = "creates 150 read-only disposable files and verifies error aggregation"]
fn failure_total_and_first_hundred() {
    let fixture = Fixture::new("acceptance-failures").unwrap();
    for via_cli in [false, true] {
        let target = fixture
            .folder(if via_cli { "cli" } else { "core" })
            .unwrap();
        for i in 0..150 {
            let path = target.join(format!("readonly-{i}.txt"));
            file(&path);
            support::readonly(&path);
        }
        if via_cli {
            let text = cli(
                &fixture,
                &["-r", "-y", "--threads", "8"],
                std::slice::from_ref(&target),
                fixture.root(),
                1,
            );
            assert_eq!(
                counts(&text).unwrap(),
                Counts {
                    files: 0,
                    directories: 0,
                    links: 0,
                    failed: 151
                }
            );
            assert_eq!(
                text.lines()
                    .filter(|line| line.starts_with("error: "))
                    .count(),
                100
            );
            assert!(text.contains("51 additional failure(s) omitted"));
        } else {
            let report = engine(std::slice::from_ref(&target), 8, false);
            assert_report(&report, 0, 0, 0, 151);
            assert_eq!(report.failures.len(), 100);
        }
        assert_eq!(fs::read_dir(target).unwrap().count(), 150);
    }
}
