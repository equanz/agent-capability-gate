use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use serde_json::json;
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::process::Command;
use std::time::Duration;

fn main() {
    // The fixture never invokes a shell or external service. It consumes stdin
    // to make EOF behavior observable and emits a stable bootstrap response.
    let mut stdin = Vec::new();
    let _ = io::stdin().read_to_end(&mut stdin);
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.iter().any(|argument| argument == "--ignore-term") {
        // This is deliberately only a fixture behavior.  The runtime must
        // still terminate this process group with SIGKILL after its grace
        // period when SIGTERM is ignored.
        #[cfg(unix)]
        unsafe {
            let action = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
            sigaction(Signal::SIGTERM, &action).expect("install SIGTERM fixture handler");
        }
    }
    if let Some(pid_file) = arguments.iter().find_map(|argument| {
        argument
            .strip_prefix("--spawn-descendant=")
            .map(str::to_owned)
    }) {
        #[allow(clippy::zombie_processes)]
        let child = Command::new(std::env::current_exe().expect("fixture executable"))
            .arg("--descendant")
            .arg(format!("--pid-file={pid_file}"))
            .spawn()
            .expect("spawn fixture descendant");
        std::fs::write(&pid_file, child.id().to_string()).expect("write descendant pid");
    }
    if let Some(delay) = arguments.iter().find_map(|argument| {
        argument
            .strip_prefix("--sleep-ms=")
            .and_then(|milliseconds| milliseconds.parse::<u64>().ok())
    }) {
        std::thread::sleep(Duration::from_millis(delay));
    }
    if arguments.iter().any(|argument| argument == "--descendant") {
        // Keep a descendant alive long enough for the parent timeout path to
        // exercise process-group cleanup.
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if let Some(pid_file) = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--pid-file="))
    {
        let _ = std::fs::write(pid_file, std::process::id().to_string());
    }
    let custom_output = arguments.iter().any(|argument| {
        argument == "--invalid-json"
            || argument.starts_with("--text=")
            || argument.starts_with("--stdout-bytes=")
            || argument.starts_with("--stderr-hex=")
    });
    if arguments
        .iter()
        .any(|argument| argument == "--invalid-json")
    {
        print!("{{not-json");
    }
    if let Some(value) = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--text="))
    {
        print!("{value}");
    }
    if let Some(bytes) = arguments.iter().find_map(|argument| {
        argument
            .strip_prefix("--stdout-bytes=")
            .and_then(|value| value.parse::<usize>().ok())
    }) {
        print!("{}", "x".repeat(bytes));
    }
    if let Some(value) = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--stderr="))
    {
        eprint!("{value}");
    }
    if let Some(bytes) = arguments.iter().find_map(|argument| {
        argument
            .strip_prefix("--stderr-bytes=")
            .and_then(|value| value.parse::<usize>().ok())
    }) {
        eprint!("{}", "e".repeat(bytes));
    }
    if let Some(encoded) = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("--stderr-hex="))
    {
        let mut bytes = Vec::with_capacity(encoded.len() / 2);
        for pair in encoded.as_bytes().chunks(2) {
            if let Ok(pair) = std::str::from_utf8(pair)
                && let Ok(byte) = u8::from_str_radix(pair, 16)
            {
                bytes.push(byte);
            }
        }
        io::stderr()
            .write_all(&bytes)
            .expect("write fixture stderr bytes");
    }
    if let Some(code) = arguments.iter().find_map(|argument| {
        argument
            .strip_prefix("--exit-code=")
            .and_then(|value| value.parse::<i32>().ok())
    }) {
        std::io::stdout().flush().ok();
        std::io::stderr().flush().ok();
        std::process::exit(code);
    }
    if custom_output {
        std::io::stdout().flush().expect("flush fixture output");
        std::io::stderr().flush().expect("flush fixture stderr");
        return;
    }
    let environment: BTreeSet<_> = std::env::vars().map(|(name, _)| name).collect();
    let observed = json!({
        "stdin_eof_observed": true,
        "argv_count": arguments.len(),
        "argv": arguments,
        "cwd": std::env::current_dir()
            .ok()
            .and_then(|path| path.to_str().map(str::to_owned)),
        "environment": environment,
    });
    println!("{observed}");
}
