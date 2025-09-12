use std::{fs, path::PathBuf};
use std::ffi::CStr;
use clap::Parser;
use clack_host::prelude::*; // PluginBundle, etc.

#[derive(Parser, Debug)]
#[command(version, about = "Hello CLAP host: list plugin descriptors")]
struct Args {
    /// Path to a .clap bundle (e.g. /usr/lib/clap/lsp-oscillator.clap)
    plugin: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let plugin_path = if let Some(p) = args.plugin {
        p
    } else {
        auto_find_plugin().unwrap_or_else(|| {
            eprintln!("No plugin path provided and auto-discovery failed.\nTry: cargo run -- /usr/lib/clap/<plugin>.clap");
            std::process::exit(2);
        })
    };

    println!("Loading bundle: {}", plugin_path.display());

    // Load the .clap bundle (unsafe: loading foreign code)
    let bundle = unsafe { PluginBundle::load(&plugin_path) }
        .map_err(|e| format!("Failed to load bundle: {e:?}"))?;

    // Get the factory and enumerate descriptors
    let factory = bundle
        .get_plugin_factory()
        .ok_or("Bundle has no plugin factory")?;

    let mut any = false;
    for desc in factory.plugin_descriptors() {
        any = true;

        // On your Clack rev, these all return Option<&CStr>
        let id      = desc.id()     .map(|c: &CStr| c.to_string_lossy().into_owned()).unwrap_or_else(|| "(no id)".into());
        let name    = desc.name()   .map(|c: &CStr| c.to_string_lossy().into_owned()).unwrap_or_else(|| "(unnamed)".into());
        let vendor  = desc.vendor() .map(|c: &CStr| c.to_string_lossy().into_owned()).unwrap_or_else(|| "(unknown vendor)".into());
        let version = desc.version().map(|c: &CStr| c.to_string_lossy().into_owned()).unwrap_or_else(|| "(unknown version)".into());

        println!("---");
        println!("ID:      {id}");
        println!("Name:    {name}");
        println!("Vendor:  {vendor}");
        println!("Version: {version}");
    }

    if !any {
        eprintln!("No descriptors found in bundle");
        std::process::exit(3);
    }

    Ok(())
}

/// Try to find a plausible synth in standard CLAP locations.
fn auto_find_plugin() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(cp) = std::env::var("CLAP_PATH") {
        dirs.extend(cp.split(':').map(PathBuf::from));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".clap"));
    }
    dirs.push(PathBuf::from("/usr/lib/clap"));

    let mut candidates = Vec::new();
    for d in dirs {
        if d.is_dir() {
            if let Ok(entries) = fs::read_dir(&d) {
                for e in entries.flatten() {
                    let p = e.path();
                    // CLAPs can be files or directories; accept both
                    if p.extension().and_then(|s| s.to_str()) == Some("clap") || p.is_dir() {
                        candidates.push(p);
                    }
                }
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }

    // Prefer names with "osc" or "synth"
    candidates.sort();
    candidates.into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|s| {
                let s = s.to_ascii_lowercase();
                s.contains("osc") || s.contains("synth")
            })
            .unwrap_or(false)
    })
}

