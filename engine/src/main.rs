mod audio;
mod channel;
mod engine;
mod pad;
mod router;

use std::io;
use std::sync::Arc;

use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::audio::decode_file;
use crate::engine::Engine;

pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("sample_rs=trace")),
        )
        .init();
    info!("Main thread starting up");

    let mut engine = Engine::start();

    let decoded_file = decode_file("assets/clap.wav").unwrap();
    println!("Decoded file: {:?}", decoded_file);
    let clap = Arc::new(decoded_file);

    let pad_id = engine.add_pad();
    let channel_id = engine.add_channel();
    engine.route_pad_to_channel(pad_id, channel_id);

    engine.load_audio(pad_id, Some(clap));

    println!("\nType '0' and press Enter to play Pad 0.");
    println!("Type 'q' and press Enter to quit.\n");

    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        input.clear();

        if stdin.read_line(&mut input).is_ok() {
            match input.trim() {
                "0" => {
                    engine.hit_pad(pad_id);
                }
                "q" => {
                    println!("Shutting down...");
                    break;
                }
                _ => {}
            }
        }
    }
}
