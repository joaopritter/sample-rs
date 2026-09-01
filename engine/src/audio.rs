use core::error;
use std::fs::File;

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, PolynomialDegree, Resampler};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    errors::Error,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
};

/// Source-agnostic audio struct, can be created
/// from a file, mic input, computer audio
/// playback, you name it. An audio is immutable.
#[derive(Debug, Clone)]
pub struct Audio {
    /// Raw audio data.
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
}

impl Audio {
    pub fn as_adapter(&self) -> InterleavedSlice<&[f32]> {
        InterleavedSlice::new(&self.data, self.channels, self.data.len() / self.channels).unwrap()
    }

    /// Resamples the audio to the target sample rate and returns a new `Audio` struct.
    pub fn resample(&self, target_sample_rate: u32) -> Self {
        let channels = self.channels;
        let (fs_in, fs_out) = (self.sample_rate, target_sample_rate);

        if fs_in == fs_out {
            return self.clone();
        }

        let nbr_input_frames = self.data.len() / channels;
        let f_ratio = fs_out as f32 / fs_in as f32;

        let estimated_out_frames = (nbr_input_frames as f32 * f_ratio).ceil() as usize;
        let mut outdata = vec![0.0f32; channels * (estimated_out_frames * 2 + 1024)];

        println!("Creating resampler...");
        let mut resampler = Async::<f32>::new_poly(
            f_ratio as f64,
            1.1,
            PolynomialDegree::Septic,
            1024,
            channels,
            FixedAsync::Input,
        )
        .unwrap();

        let mut input_frames_next = resampler.input_frames_next();
        let resampler_delay = resampler.output_delay();

        let input_adapter = self.as_adapter();
        let outdata_capacity = outdata.len() / channels;
        let mut output_adapter =
            InterleavedSlice::new_mut(&mut outdata, channels, outdata_capacity).unwrap();

        let mut indexing = Indexing::new();
        let mut input_frames_left = nbr_input_frames;

        while input_frames_left >= input_frames_next {
            let (nbr_in, nbr_out) = resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .unwrap();

            indexing.input_offset += nbr_in;
            indexing.output_offset += nbr_out;
            input_frames_left -= nbr_in;
            input_frames_next = resampler.input_frames_next();
        }

        if input_frames_left > 0 {
            indexing.partial_len = Some(input_frames_left);
            let (_nbr_in, nbr_out) = resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .unwrap();

            indexing.output_offset += nbr_out;
        }

        let nbr_output_frames = (nbr_input_frames as f32 * f_ratio) as usize;

        let first = resampler_delay * channels;
        let last = (first + nbr_output_frames * channels).min(outdata.len());
        let trimmed_data = outdata[first..last].to_vec();

        Self {
            data: trimmed_data,
            sample_rate: fs_out,
            channels,
        }
    }
}

// TODO: Make a trait to used as "SoundSource", so both memory loaded audio
// and real time calculated waves can be used as Audio by the engine.

pub fn decode_file(path: &std::path::Path) -> Result<Audio, Box<dyn error::Error>> {
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
        channels: channels as usize,
    })
}
