//! Encode a Little Printer payload and write the raw bytes to stdout.
//!
//! The bytes are what Sirius base64-encodes as `binary_payload` in a
//! DeviceCommand, which is what the Bridge daemon's `executeCommand` consumes.

mod encode;
mod png_in;

use std::io::Write;

fn main() {
    if let Err(e) = run() {
        eprintln!("lp-encode: {e}");
        std::process::exit(1);
    }
}

fn usage() -> String {
    let mut s = String::from("usage: lp-encode [--stats] <command> <print-id> <png>...\n");
    for (name, opcode, images) in encode::COMMANDS {
        s += &format!("  {name:<30} opcode {opcode:#06x}  {images} image(s)\n");
    }
    s += "\nEach PNG must be exactly 384 px wide and already pure black and white.\n";
    s
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let stats = args.first().map(|a| a == "--stats").unwrap_or(false);
    if stats {
        args.remove(0);
    }
    if args.len() < 3 {
        return Err(usage());
    }

    let (_, opcode, want) = encode::COMMANDS
        .iter()
        .find(|(n, _, _)| *n == args[0])
        .ok_or_else(|| format!("unknown command {:?}\n\n{}", args[0], usage()))?;

    let print_id: u32 = args[1]
        .strip_prefix("0x")
        .map(|h| u32::from_str_radix(h, 16))
        .unwrap_or_else(|| args[1].parse())
        .map_err(|e| format!("print-id {:?}: {e}", args[1]))?;

    let paths = &args[2..];
    if paths.len() != *want {
        return Err(format!(
            "{} needs {want} image(s), got {}",
            args[0],
            paths.len()
        ));
    }

    let start = std::time::Instant::now();
    let mut images = Vec::with_capacity(paths.len());
    for p in paths {
        images.push(png_in::load_bw(std::path::Path::new(p))?.0);
    }
    let out = encode::encode_command(*opcode, print_id, &images)?;
    let elapsed = start.elapsed();

    std::io::stdout()
        .write_all(&out)
        .map_err(|e| format!("stdout: {e}"))?;

    if stats {
        // Sampled from inside the process: it exits too fast to catch from outside.
        let hwm = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmHWM:"))
                    .map(|l| l["VmHWM:".len()..].trim().to_string())
            })
            .unwrap_or_else(|| "unavailable".into());
        eprintln!(
            "bytes={} elapsed_ms={} VmHWM={}",
            out.len(),
            elapsed.as_millis(),
            hwm
        );
    }
    Ok(())
}
