use std::io;

use cpal::traits::HostTrait;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use sample_rs_engine::Engine;
use tracing::info;
use tracing_subscriber::EnvFilter;

pub fn main() -> io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("sample_rs=debug")),
        )
        .init();
    info!("Main thread starting up");

    let host = cpal::default_host();
    let device = host.default_output_device().expect("No device");
    let mut engine = Engine::start(device);

    let file = std::path::Path::new("assets/clap.wav");
    let audio_id = engine.load_file(file);
    let clap = engine.get_audio(audio_id);

    let pad_id = engine.add_pad();
    let channel_id = engine.add_channel();

    engine.update_pad(pad_id, |props| {
        props.set_target_channel(channel_id);
        props.set_audio(Some(clap.clone()));
    });

    println!("\nPress 'a' to play Pad {}.", pad_id);
    println!("Press 'q' or Ctrl+C to quit.\n");

    enable_raw_mode()?;

    loop {
        if let Ok(Event::Key(key_event)) = event::read()
            && key_event.kind == KeyEventKind::Press
        {
            match key_event.code {
                KeyCode::Char('a') => {
                    engine.hit_pad(pad_id);
                }
                KeyCode::Char('o') => {
                    engine.update_pad(pad_id, |props| {
                        props.offset_volume(-0.1);
                    });
                }
                KeyCode::Char('p') => {
                    engine.update_pad(pad_id, |props| {
                        props.offset_volume(0.1);
                    });
                }
                KeyCode::Char('q') => {
                    break;
                }
                KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    break;
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;

    println!("Shutting down...");
    Ok(())
}
