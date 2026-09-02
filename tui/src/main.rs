use std::io::{self, Write};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use sample_rs_engine::{BufferSize, Channels, Context, Engine, SampleRate};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

pub struct RawModeSanitizer<W> {
    inner: W,
}

impl<W: Write> Write for RawModeSanitizer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut last = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' {
                self.inner.write_all(&buf[last..i])?;
                self.inner.write_all(b"\r\n")?;
                last = i + 1;
            }
        }
        self.inner.write_all(&buf[last..])?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Clone, Default)]
pub struct RawModeMakeWriter;

impl<'a> MakeWriter<'a> for RawModeMakeWriter {
    type Writer = RawModeSanitizer<io::Stdout>;

    fn make_writer(&'a self) -> Self::Writer {
        RawModeSanitizer {
            inner: io::stdout(),
        }
    }
}

pub fn main() -> io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("sample_rs=trace")),
        )
        .with_writer(RawModeMakeWriter)
        .init();
    info!("Main thread starting up");

    let host = cpal::default_host();
    let device = host.default_output_device().expect("No device");

    let context = Context::new(SampleRate::R48kHz, BufferSize::S512, Channels::Stereo);

    let desired_buffer_size = context.buffer_size();
    let supported_config = device.default_output_config().unwrap();

    match supported_config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => {
            if *min > desired_buffer_size || *max < desired_buffer_size {
                panic!()
            }
        }
        cpal::SupportedBufferSize::Unknown => todo!(),
    }

    let mut stream_config = supported_config.config();
    stream_config.buffer_size = cpal::BufferSize::Fixed(desired_buffer_size);
    stream_config.sample_rate = context.sample_rate();

    let (mut engine, mut audio_out) = Engine::start(context);

    let mut error_logged = false;

    let stream = device
        .build_output_stream(
            stream_config,
            move |data: &mut [f32], _| {
                audio_out.feed(data);
            },
            move |err| {
                if !error_logged {
                    eprintln!("Audio stream error occurred: {:#?}", err);
                    error_logged = true
                }
            },
            None,
        )
        .unwrap();

    stream.play().unwrap();

    let clap_id = engine.load_file(std::path::Path::new("assets/clap.wav"));
    let clap = engine.get_audio(clap_id);

    let pad_id = engine.add_pad();
    let channel_id = engine.add_channel();

    engine.update_pad(pad_id, |props| {
        props.set_target_channel(channel_id);
        props.set_audio(Some(clap.clone()));
    });

    let kick_id = engine.load_file(std::path::Path::new("assets/kick.wav"));
    let kick = engine.get_audio(kick_id);

    let second_pad_id = engine.add_pad();
    let second_channel_id = engine.add_channel();

    engine.update_pad(second_pad_id, |props| {
        props.set_target_channel(second_channel_id);
        props.set_audio(Some(kick.clone()));
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
                KeyCode::Char('s') => {
                    engine.hit_pad(second_pad_id);
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
