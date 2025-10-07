use std::{path::PathBuf};
use clap::Parser;

use clack_host::prelude::*;
use clack_host::events::io::{InputEvents, OutputEvents, EventBuffer};
use clack_host::prelude::UnknownEvent;

use jack::{Client, ClientOptions, Control, ProcessHandler, ProcessScope, AudioOut, Port};

#[derive(Parser, Debug)]
#[command(version, about = "CLAP -> JACK: run LSP Noise Generator through JACK")]
struct Args {
    /// Path to a .clap bundle (e.g. /usr/lib/clap/lsp-plugins.clap)
    plugin: PathBuf,
}

/* ------- minimal clack host scaffolding ------- */
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
/* --------------------------------------------- */

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let plugin_path = args.plugin;

    println!("Loading bundle: {}", plugin_path.display());

    // Load bundle (FFI boundary)
    let bundle = unsafe { PluginBundle::load(&plugin_path) }
        .map_err(|e| format!("Failed to load bundle: {e:?}"))?;

    let factory = bundle
        .get_plugin_factory()
        .ok_or("Bundle has no plugin factory")?;

    // Choose a generator that needs no MIDI
    let target_id = "in.lsp-plug.noise_generator_x1";
    let mut target_desc = None;
    for d in factory.plugin_descriptors() {
        if let Some(id) = d.id() {
            if id.to_string_lossy() == target_id {
                target_desc = Some(d);
                break;
            }
        }
    }
    let Some(desc) = target_desc else {
        eprintln!("Could not find {target_id} in this bundle.");
        std::process::exit(3);
    };

    println!("Instantiating {target_id}…");

    // Host identity (name, vendor, url, version)
    let host_info = HostInfo::new(
        "jack_minimal_clap",
        "Giles",
        "https://example.invalid",
        "0.1.0",
    )?;

    // Create instance
    let mut instance = PluginInstance::<MyHost>::new(
        |_| MyHostShared,
        |_| (),
        &bundle,
        desc.id().expect("descriptor must have id"),
        &host_info,
    )?;

    // Open JACK first to use its real SR / block size
    let (jack_client, _status) = Client::new("clap_to_jack", ClientOptions::NO_START_SERVER)
        .expect("JACK not available");
    let sample_rate = jack_client.sample_rate() as f64;
    let frames      = jack_client.buffer_size() as u32;
    println!("JACK: sr={sample_rate}, buffer={frames}");

    // Activate plugin with JACK params
    let audio_cfg = PluginAudioConfiguration {
        sample_rate,
        min_frames_count: frames,
        max_frames_count: frames,
    };
    let audio_proc_stopped = instance.activate(|_, _| (), audio_cfg)?;
    let audio_proc_started = audio_proc_stopped.start_processing()?;

    // Register JACK outs
    let out_l = jack_client.register_port("out_l", AudioOut::default()).expect("jack L");
    let out_r = jack_client.register_port("out_r", AudioOut::default()).expect("jack R");

    // Move processor into handler
    let handler = JackHandler {
        proc: audio_proc_started,
        out_l,
        out_r,
        in_l: Vec::new(),
        in_r: Vec::new(),
        scratch_l: Vec::new(),
        scratch_r: Vec::new(),
    };
    let _active = jack_client.activate_async((), handler).expect("activate JACK failed");

    println!("Running. Connect to playback, e.g.:");
    println!("  jack_connect \"clap_to_jack:out_l\" \"USB Audio Analog Stereo:playback_FL\"");
    println!("  jack_connect \"clap_to_jack:out_r\" \"USB Audio Analog Stereo:playback_FR\"");
    println!("Ctrl+C to quit.");
    loop { std::thread::park(); }
}

// JACK handler that calls the CLAP plugin each block
struct JackHandler {
    proc: clack_host::process::StartedPluginAudioProcessor<MyHost>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    // silent input we'll hand to the plugin
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    // plugin output scratch (copied to JACK)
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl ProcessHandler for JackHandler {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        let out_l = self.out_l.as_mut_slice(ps);
        let out_r = self.out_r.as_mut_slice(ps);
        let n = out_l.len();

        // Ensure buffers are the right size
        if self.in_l.len() != n { self.in_l.resize(n, 0.0); }
        if self.in_r.len() != n { self.in_r.resize(n, 0.0); }
        if self.scratch_l.len() != n { self.scratch_l.resize(n, 0.0); }
        if self.scratch_r.len() != n { self.scratch_r.resize(n, 0.0); }

        // Build clack audio ports: 1 input port (silent stereo), 1 output port (stereo)
        let mut input_ports  = AudioPorts::with_capacity(2, 1);
        let mut output_ports = AudioPorts::with_capacity(2, 1);

        // Explicitly-typed EMPTY input event buffer — slice of references
        let empty_in: [&UnknownEvent; 0] = [];
        let input_events = InputEvents::from_buffer(&empty_in);
        let mut output_events_buf = EventBuffer::new();
        let mut output_events = OutputEvents::from_buffer(&mut output_events_buf);

        // Attach input (silent stereo) and output (our scratch) buffers
        let mut in_audio = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                // IMPORTANT: pass **mutable** slices to InputChannel::constant(...)
                [&mut self.in_l[..], &mut self.in_r[..]]
                    .into_iter()
                    .map(InputChannel::constant)
            )
        }]);
        let mut out_audio = output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                [&mut self.scratch_l[..], &mut self.scratch_r[..]].into_iter()
            )
        }]);

        // Process one JACK block
        let _status = self.proc.process(
            &in_audio,
            &mut out_audio,
            &input_events,
            &mut output_events,
            None,
            None
        ).unwrap_or(ProcessStatus::Continue);

        // Copy to JACK
        out_l.copy_from_slice(&self.scratch_l);
        out_r.copy_from_slice(&self.scratch_l);

        Control::Continue
    }
}
