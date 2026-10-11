#[cfg(windows)]
#[path = "cancellation/windows.rs"]
mod windows;

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows::run() {
        eprintln!("Cancellation validation failed: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    println!("SKIPPED cancellation: Windows console APIs are required");
}
