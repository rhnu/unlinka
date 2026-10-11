//! Explorer adapters copy selected filesystem paths and hand them to the desktop task.

#[cfg(all(windows, not(target_arch = "x86_64")))]
compile_error!("The Explorer extension supports native x64 Windows only");

#[cfg(windows)]
mod selection;

#[cfg(windows)]
mod server;
