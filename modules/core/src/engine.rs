use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};

use crate::{
    DeleteOptions, DeleteReport, Failure, PreparedTargets, Progress,
    diagnostic::{DiagnosticCode, error},
    paths::{Protection, pin_ancestors},
    win::{Finder, Handle, Kind},
};

struct Run<'a> {
    options: &'a DeleteOptions,
    cancelled: &'a AtomicBool,
    report: Mutex<DeleteReport>,
    // Populated before workers start and kept alive through their completion.
    ancestors: Vec<Handle>,
}

struct MonitorStop<'a>(&'a AtomicBool);

impl Drop for MonitorStop<'_> {
    fn drop(&mut self) { self.0.store(false, Ordering::Release); }
}

impl Run<'_> {
    fn stopped(&self) -> bool { self.cancelled.load(Ordering::Acquire) }

    fn fail(&self, path: PathBuf, operation: &'static str, error: io::Error) {
        self.report
            .lock()
            .expect("report lock poisoned")
            .fail(path, operation, error);
    }

    fn verify_ancestors(&self) -> io::Result<()> {
        for handle in &self.ancestors {
            handle.require_directory()?;
        }
        Ok(())
    }

    fn remove(&self, path: &std::path::Path, handle: Handle, kind: Kind) {
        if self.stopped() {
            return;
        }
        let result = if self.options.dry_run {
            Ok(())
        } else {
            handle.remove()
        };
        // Close before allowing the parent to become deletable (legacy delete
        // becomes effective on close). Never reopen a name to delete it.
        drop(handle);
        match result {
            Ok(()) => self
                .report
                .lock()
                .expect("report lock poisoned")
                .count(kind),
            Err(error) => self.fail(path.to_owned(), "delete", error),
        }
    }
}

struct Directory {
    path: PathBuf,
    handle: RwLock<Option<Handle>>,
    parent: Option<Arc<Self>>,
    // One scanning dependency plus one for each scheduled child.
    pending: AtomicUsize,
    scan_failed: AtomicBool,
}

impl Directory {
    fn new(path: PathBuf, handle: Handle, parent: Option<Arc<Self>>) -> Arc<Self> {
        Arc::new(Self {
            path,
            handle: RwLock::new(Some(handle)),
            parent,
            pending: AtomicUsize::new(1),
            scan_failed: AtomicBool::new(false),
        })
    }

    fn add_child(&self) { self.pending.fetch_add(1, Ordering::Relaxed); }

    fn verify_chain(&self, run: &Run<'_>) -> io::Result<()> {
        run.verify_ancestors()?;
        let mut current = Some(self);
        while let Some(directory) = current {
            directory
                .handle
                .read()
                .expect("directory lock poisoned")
                .as_ref()
                .ok_or_else(|| io::Error::other("parent directory already completed"))?
                .require_directory()?;
            current = directory.parent.as_deref();
        }
        Ok(())
    }

    fn complete(mut directory: Arc<Self>, run: &Run<'_>) {
        // Iterative propagation avoids a call stack proportional to tree depth.
        loop {
            if directory.pending.fetch_sub(1, Ordering::AcqRel) != 1 {
                return;
            }
            let handle = directory
                .handle
                .write()
                .expect("directory lock poisoned")
                .take()
                .expect("directory completed only once");
            if directory.scan_failed.load(Ordering::Acquire) {
                drop(handle);
            } else {
                run.remove(&directory.path, handle, Kind::Directory);
            }
            let Some(parent) = directory.parent.clone() else {
                return;
            };
            directory = parent;
        }
    }
}

struct Leaf {
    path: PathBuf,
    // Top-level entries use the already verified handle; internal leaves are
    // opened by workers while their pinned parent keeps the path anchored.
    handle: Option<Handle>,
    parent: Option<Arc<Directory>>,
}

fn process(leaf: Leaf, run: &Run<'_>) {
    if !run.stopped() {
        let opened = leaf
            .parent
            .as_ref()
            .map_or(Ok(()), |parent| parent.verify_chain(run))
            .and_then(|()| {
                leaf.handle
                    .map_or_else(|| Handle::open(&leaf.path, !run.options.dry_run, false), Ok)
            });
        match opened.and_then(|handle| {
            let kind = handle.kind()?;
            if kind == Kind::Directory {
                return Err(error(
                    DiagnosticCode::DirectoryChanged,
                    "leaf became an ordinary directory; refusing to traverse it",
                ));
            }
            Ok((handle, kind))
        }) {
            Ok((handle, kind)) => run.remove(&leaf.path, handle, kind),
            Err(error) => run.fail(leaf.path.clone(), "open", error),
        }
    }
    if let Some(parent) = leaf.parent {
        Directory::complete(parent, run);
    }
}

fn worker(receiver: &Receiver<Leaf>, run: &Run<'_>) {
    while let Ok(leaf) = receiver.recv() {
        process(leaf, run);
    }
}

fn schedule(mut leaf: Leaf, sender: Option<&Sender<Leaf>>, run: &Run<'_>) {
    let Some(sender) = sender else {
        process(leaf, run);
        return;
    };
    loop {
        if run.stopped() {
            process(leaf, run);
            return;
        }
        match sender.send_timeout(leaf, Duration::from_millis(25)) {
            Ok(()) => return,
            Err(SendTimeoutError::Timeout(returned)) => leaf = returned,
            Err(SendTimeoutError::Disconnected(returned)) => {
                run.fail(
                    returned.path.clone(),
                    "schedule",
                    io::Error::other("deletion workers disconnected"),
                );
                if let Some(parent) = returned.parent {
                    Directory::complete(parent, run);
                }
                return;
            },
        }
    }
}

struct Frame {
    directory: Arc<Directory>,
    finder: Option<Finder>,
}

fn frame(directory: Arc<Directory>, run: &Run<'_>) -> Frame {
    let finder = match directory
        .verify_chain(run)
        .and_then(|()| Finder::open(&directory.path))
    {
        Ok(finder) => finder,
        Err(error) => {
            directory.scan_failed.store(true, Ordering::Release);
            run.fail(directory.path.clone(), "scan", error);
            None
        },
    };
    Frame { directory, finder }
}

fn scan(root: Arc<Directory>, sender: Option<&Sender<Leaf>>, run: &Run<'_>) {
    let mut stack = vec![frame(root, run)];
    while !stack.is_empty() {
        if run.stopped() {
            while let Some(frame) = stack.pop() {
                drop(frame.finder);
                Directory::complete(frame.directory, run);
            }
            return;
        }
        let current = stack.last_mut().expect("nonempty scan stack");
        let entry = current
            .directory
            .verify_chain(run)
            .and_then(|()| current.finder.as_mut().map_or(Ok(None), Finder::next));
        let (name, directory_hint) = match entry {
            Ok(Some(entry)) => entry,
            result => {
                if let Err(error) = result {
                    current.directory.scan_failed.store(true, Ordering::Release);
                    run.fail(current.directory.path.clone(), "scan", error);
                }
                let done = stack.pop().expect("nonempty scan stack");
                drop(done.finder);
                Directory::complete(done.directory, run);
                continue;
            },
        };
        let parent = Arc::clone(&current.directory);
        let path = parent.path.join(name);
        if directory_hint {
            // Open reparse entries themselves, then decide from the handle;
            // enumeration attributes never authorize a traversal.
            match parent
                .verify_chain(run)
                .and_then(|()| Handle::open(&path, !run.options.dry_run, true))
                .and_then(|handle| Ok((handle.kind()?, handle)))
            {
                Ok((Kind::Directory, handle)) => {
                    parent.add_child();
                    stack.push(frame(Directory::new(path, handle, Some(parent)), run));
                },
                Ok((_, handle)) => {
                    parent.add_child();
                    schedule(
                        Leaf {
                            path,
                            handle: Some(handle),
                            parent: Some(parent),
                        },
                        sender,
                        run,
                    );
                },
                Err(error) => run.fail(path, "open-directory", error),
            }
        } else {
            parent.add_child();
            schedule(
                Leaf {
                    path,
                    handle: None,
                    parent: Some(parent),
                },
                sender,
                run,
            );
        }
    }
}

struct Root {
    path: PathBuf,
    handle: Handle,
    kind: Kind,
}

struct ValidatedRoots {
    roots: Vec<Root>,
    ancestors: Vec<Handle>,
}

fn validate_roots(targets: PreparedTargets, run: &Run<'_>) -> Result<ValidatedRoots, Failure> {
    let protection = Protection::collect().map_err(|error| Failure {
        path: PathBuf::new(),
        operation: "validate-protection",
        error,
    })?;
    let mut pins = Vec::new();
    let mut roots = Vec::with_capacity(targets.entries.len());
    for entry in targets.entries {
        if run.stopped() {
            return Ok(ValidatedRoots {
                roots: Vec::new(),
                ancestors: pins,
            });
        }
        let (handle, identity) = (|| -> io::Result<_> {
            pins.extend(pin_ancestors(&entry.path)?);
            let handle = Handle::open(
                &entry.path,
                !run.options.dry_run,
                entry.identity.kind == Kind::Directory,
            )?;
            let identity = handle.identity()?;
            if identity != entry.identity {
                return Err(error(
                    DiagnosticCode::IdentityChanged,
                    "target identity or type changed after confirmation",
                ));
            }
            protection.check(&handle.final_path()?, identity)?;
            if identity.kind == Kind::Directory && !run.options.recursive {
                return Err(error(
                    DiagnosticCode::RecursiveRequired,
                    "ordinary directory requires recursive mode",
                ));
            }
            Ok((handle, identity))
        })()
        .map_err(|error| Failure {
            path: entry.path.clone(),
            operation: "validate-target",
            error,
        })?;
        // Snapshot must close before legacy disposition can finish deletion.
        drop(entry.snapshot);
        roots.push(Root {
            path: entry.path,
            handle,
            kind: identity.kind,
        });
    }
    Ok(ValidatedRoots {
        roots,
        ancestors: pins,
    })
}

pub(crate) fn execute(
    targets: PreparedTargets,
    options: &DeleteOptions,
    cancelled: &AtomicBool,
    observer: Option<&(dyn Fn(Progress) + Sync)>,
) -> DeleteReport {
    let start = Instant::now();
    let mut run = Run {
        options,
        cancelled,
        report: Mutex::new(DeleteReport::default()),
        ancestors: Vec::new(),
    };
    if !(1..=64).contains(&options.threads) {
        run.fail(
            PathBuf::new(),
            "options",
            error(
                DiagnosticCode::InvalidOptions,
                "threads must be between 1 and 64",
            ),
        );
    } else if !run.stopped() {
        match validate_roots(targets, &run) {
            Err(failure) => run.fail(failure.path, failure.operation, failure.error),
            Ok(ValidatedRoots { roots, ancestors }) => {
                run.ancestors = ancestors;
                // All roots are verified and pinned before the first mutation.
                let monitoring = AtomicBool::new(true);
                thread::scope(|monitor_scope| {
                    if let Some(observer) = observer {
                        let report = &run.report;
                        let monitoring = &monitoring;
                        monitor_scope.spawn(move || {
                            while monitoring.load(Ordering::Acquire) {
                                let progress = {
                                    let report = report.lock().expect("report lock poisoned");
                                    Progress {
                                        files: report.files,
                                        directories: report.directories,
                                        links: report.links,
                                        failed: report.failed,
                                        elapsed: start.elapsed(),
                                    }
                                };
                                observer(progress);
                                thread::sleep(Duration::from_millis(100));
                            }
                        });
                    }
                    let stop_monitor = MonitorStop(&monitoring);
                    // Worker scope ends before the monitor is stopped so the last
                    // returned report includes every in-flight operation.
                    thread::scope(|worker_scope| {
                        let (sender, receiver) =
                            bounded((options.threads.saturating_sub(1) * 2).max(1));
                        for _ in 1..options.threads {
                            let receiver = receiver.clone();
                            let run = &run;
                            worker_scope.spawn(move || worker(&receiver, run));
                        }
                        // Only workers retain receivers. If all of them unwind,
                        // the scanner observes disconnection instead of waiting
                        // forever on a full queue while the scopes try to join.
                        drop(receiver);
                        let queue = (options.threads > 1).then_some(&sender);
                        for Root { path, handle, kind } in roots {
                            if run.stopped() {
                                break;
                            }
                            if kind == Kind::Directory {
                                scan(Directory::new(path, handle, None), queue, &run);
                            } else {
                                schedule(
                                    Leaf {
                                        path,
                                        handle: Some(handle),
                                        parent: None,
                                    },
                                    queue,
                                    &run,
                                );
                            }
                        }
                        drop(sender);
                    });
                    drop(stop_monitor);
                });
            },
        }
    }
    let mut report = run.report.into_inner().expect("report lock poisoned");
    report.cancelled = cancelled.load(Ordering::Acquire);
    report.elapsed = start.elapsed();
    report
}
