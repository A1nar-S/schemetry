use std::fs::OpenOptions;
use std::io::Write;

const CRASH_LOG_FILE: &str = "crash.log";

/// Logs panic details to `crash.log` — release builds abort with no console,
/// so without this a crash would just be the app silently disappearing.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());

        let entry =
            format!("[{timestamp}] thread '{thread}' panicked at {location}:\n{message}\n\n");

        eprintln!("{entry}");

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(CRASH_LOG_FILE)
        {
            let _ = file.write_all(entry.as_bytes());
        }
    }));
}
