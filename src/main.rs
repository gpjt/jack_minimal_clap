use std::{fs, path::PathBuf};
use std::ffi::CStr;
use clap::Parser;
use clack_host::prelude::*;
use clack_host::events::event_types::*; // (not used yet, but handy next step)

#[derive(Parser, Debug)]
#[command(version, about = "Hello CLAP host: list plugin descriptors and instantiate LSP Oscillator")]
struct Args {
    /// Path to a .clap bundle (e.g. /usr/lib/clap/lsp-plugins.clap)
    plugin: Option<PathBuf>,
}

/* --- Minimal host scaffolding required by clack --- */
struct MyHostShared;
impl<'a> SharedHandler<'a> for MyHostShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}
struct MyHost;
impl HostHandlers for MyHost {
    type Shared<'a> = MyHostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}
/* -------------------------------------------------- */

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

    // ---- Instantiate the LSP Oscillator Mono using the current clack API ----
    let target_id = "in.lsp-plug.oscillator_mono";
    let mut target_desc = None;
    for desc in factory.plugin_descriptors() {
        if let Some(id) = desc.id() {
            if id.to_string_lossy() == target_id {
                target_desc = Some(desc);
                break;
            }
        }
    }

    if let Some(desc) = target_desc {
        println!("\nInstantiating {target_id}…");

        // Host identity now needs (name, vendor, url, version) and returns Result.
        let host_info = HostInfo::new(
            "jack_minimal_clap",
            "Giles",
            "https://example.invalid",  // arbitrary URL
            "0.1.0",
        )?;

        // Create a PluginInstance via clack’s high-level helper.
        let mut instance = PluginInstance::<MyHost>::new(
            |_| MyHostShared,             // construct Shared
            |_| (),                       // construct MainThread
            &bundle,
            desc.id().expect("descriptor must have an id"),
            &host_info,
        )?;

        // Activate with a plausible audio config (we’ll wire real JACK values later).
        let audio_cfg = PluginAudioConfiguration {
            sample_rate: 48_000.0,
            min_frames_count: 256,
            max_frames_count: 512,
        };
        let audio_proc = instance.activate(|_, _| (), audio_cfg)?; // () for AudioProcessor state

        // Start/stop just to prove the lifecycle works.
        let audio_proc = audio_proc.start_processing()?;
        let audio_proc = audio_proc.stop_processing();
        instance.deactivate(audio_proc);

        println!("Instance created and activated successfully ✅");
    } else {
        eprintln!("\nCould not find {target_id} in this bundle.");
    }
    // ------------------------------------------------------------------------

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
