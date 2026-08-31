use std::error;
use std::fs::File;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Source-agnostic audio struct, can be created
/// from a file, mic input, computer audio
/// playback, you name it. An audio is immutable.
#[derive(Debug)]
pub struct Audio {
    /// Raw audio data.
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

// TODO: Make a trait to used as "SoundSource", so both memory loaded audio
// and real time calculated waves can be used as Audio by the engine.

pub fn decode_file(path: &str) -> Result<Audio, Box<dyn error::Error>> {
    let src = File::open(path).expect("Failed to open audio file. Does it exist?");
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(ext);
    }

    let fmt_opts: FormatOptions = Default::default();
    let meta_opts: MetadataOptions = Default::default();

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, fmt_opts, meta_opts)
        .expect("Unsupported format");

    let track = format
        .default_track(TrackType::Audio)
        .expect("No supported audio tracks");
    let track_id = track.id;

    let audio_params = track
        .codec_params
        .as_ref()
        .expect("Missing codec params")
        .audio()
        .expect("Not an audio track");

    let sample_rate = audio_params.sample_rate.unwrap_or(44100);
    let channels = audio_params
        .channels
        .as_ref()
        .map(|c| c.count() as u32)
        .unwrap_or(2);

    let dec_opts: AudioDecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_params, &dec_opts)
        .expect("Unsupported codec");

    let mut data = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => {
                eprintln!("Decode error: {}", err);
                break;
            }
        };

        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let sample_count = audio_buf.samples_interleaved();
                let start_len = data.len();

                data.resize(start_len + sample_count, 0.0);
                audio_buf.copy_to_slice_interleaved(&mut data[start_len..]);
            }
            Err(Error::DecodeError(_)) => continue, // Recoverable error, skip packet
            Err(e) => panic!("Fatal decode error: {}", e),
        }
    }

    Ok(Audio {
        data,
        sample_rate,
        channels: channels as u16,
    })
}
