use audioadapter::{Adapter, AdapterMut};
use audioadapter_buffers::direct::InterleavedSlice;
use ringbuf::{
    HeapCons, HeapProd, HeapRb, traits::{Consumer, Observer, Producer, Split},
};
use std::sync::Arc;
use tracing::{debug, instrument, trace};
use triple_buffer::{Input, Output, TripleBuffer};

use crate::audio::Audio;

struct Voice {
    index: usize,
}

#[derive(Debug, Clone)]
pub struct PadProperties {
    id: usize,
    p_audio: Option<Arc<Audio>>,
    volume: f32,
    target_channel: usize,
}

impl PadProperties {
    pub fn target_channel(&self) -> usize {
        self.target_channel
    }

    pub fn set_target_channel(&mut self, channel_id: usize) {
        self.target_channel = channel_id;
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn set_audio(&mut self, audio: Option<Arc<Audio>>) {
        self.p_audio = audio;
    }

    #[instrument(skip_all, fields(pad_id = self.id()))]
    pub fn offset_volume(&mut self, amount: f32) {
        self.volume = (self.volume + amount).clamp(0.0, 1.0);
        debug!("Volume changed to: {}", self.volume);
    }
}

impl PadProperties {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            ..Default::default()
        }
    }
}

impl Default for PadProperties {
    fn default() -> Self {
        Self {
            id: 1,
            p_audio: None,
            volume: 1.0,
            target_channel: 0, // 0 should be master track.
        }
    }
}

pub struct PadEngine {
    properties: Output<PadProperties>,
    voices: Vec<Voice>,
    producer: HeapProd<f32>,
    mix_buffer: Vec<f32>,
}

impl PadEngine {
    const MAX_CHUNK_SIZE: usize = 1024;
    const MAX_VOICES: usize = 32;

    pub fn new(id: usize) -> (Self, HeapCons<f32>, Input<PadProperties>) {
        let (pad_in, pad_out) = HeapRb::<f32>::new(8192).split();
        let (prop_w, prop_r) = TripleBuffer::new(&PadProperties::new(id)).split();

        (
            Self {
                properties: prop_r,
                voices: Vec::with_capacity(Self::MAX_VOICES),
                producer: pad_in,
                mix_buffer: vec![0.0; Self::MAX_CHUNK_SIZE],
            },
            pad_out,
            prop_w,
        )
    }

    pub fn target_channel(&self) -> usize {
        self.properties.output_buffer().target_channel()
    }

    pub fn id(&self) -> usize {
        self.properties.output_buffer().id()
    }

    pub fn spawn_voice(&mut self) {
        if self.voices.len() < self.voices.capacity() {
            self.voices.push(Voice { index: 0 });
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    pub fn has_active_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    #[instrument(skip_all, fields(pad_id = self.id()))]
    pub fn process(&mut self, channels: usize) -> bool {
        self.properties.update();
        let properties = self.properties.output_buffer();

        if self.voices.is_empty() {
            trace!("No voices to process...");
            return false;
        }

        let audio = match properties.p_audio.as_ref() {
            Some(a) => {
                trace!("Has audio loaded...");
                a
            }
            None => {
                trace!("No audio loaded, exiting.");
                return false;
            }
        };

        let available_samples = self.producer.vacant_len();
        if available_samples < channels {
            trace!("Producer has no vacant frames in buffer, exiting.");
            return false;
        }

        let available_frames = available_samples / channels;
        let out_frames = available_frames.min(Self::MAX_CHUNK_SIZE);
        let chunk_samples = out_frames * channels;

        trace!(
            "Rendering {} frames ({} samples).",
            out_frames, chunk_samples
        );

        if self.mix_buffer.len() < chunk_samples {
            self.mix_buffer.resize(chunk_samples, 0.0);
        }

        let buffer = &mut self.mix_buffer[..chunk_samples];
        buffer.fill(0.0);

        let mut dst_adapter = InterleavedSlice::new_mut(buffer, channels, out_frames).unwrap();
        let src_adapter = audio.as_adapter();

        let vol = properties.volume;

        for voice in &mut self.voices {
            let remain_frames = src_adapter.frames().saturating_sub(voice.index);
            let process_frames = out_frames.min(remain_frames);

            for ch in 0..channels {
                let src_ch = ch.min(src_adapter.channels() - 1);

                for f in 0..process_frames {
                    let sample = src_adapter.read_sample(src_ch, voice.index + f).unwrap();
                    let current = dst_adapter.read_sample(ch, f).unwrap();

                    dst_adapter
                        .write_sample(ch, f, &(current + (sample * vol)))
                        .unwrap();
                }
            }

            voice.index += process_frames;
        }

        let audio_frames = src_adapter.frames();
        self.voices.retain(|v| v.index < audio_frames);

        self.producer.push_slice(buffer);
        true
    }
}

pub enum PadManagerCommand {
    Hit(usize),
    AddPad(PadEngine),
}

pub struct PadManager {
    pads: Vec<PadEngine>,
    cmd_rx: ringbuf::HeapCons<PadManagerCommand>,
}

impl PadManager {
    pub fn new() -> (Self, ringbuf::HeapProd<PadManagerCommand>) {
        let (cmd_tx, cmd_rx) = ringbuf::HeapRb::<PadManagerCommand>::new(25).split();
        (
            Self {
                pads: Vec::new(),
                cmd_rx,
            },
            cmd_tx,
        )
    }

    /// Returns which pads are currently playing and where to route them to.
    /// impl Iterator is favored as return instead of Vec so there's no memory
    /// allocation during audio rendering, everything should be pre allocated
    /// beforehand.
    pub fn process(&mut self, channels: usize) -> impl Iterator<Item = (usize, usize)> {
        while let Some(cmd) = self.cmd_rx.try_pop() {
            match cmd {
                PadManagerCommand::Hit(p_id) => {
                    trace!("Triggering {}", p_id);
                    for p in &mut self.pads {
                        if p.id() == p_id {
                            p.spawn_voice();
                        }
                    }
                }
                PadManagerCommand::AddPad(pad_engine) => {
                    trace!("Adding pad '{}' to pad manager.", pad_engine.id());
                    self.pads.push(pad_engine);
                }
            }
        }

        for p in &mut self.pads {
            p.process(channels);
            trace!(
                "Pad '{}' has '{}' active voices.",
                p.id(),
                p.active_voices()
            );
        }

        self.pads.iter().filter_map(|pad| {
            if pad.has_active_voices() {
                trace!(
                    "Routing pad '{}' to channel '{}'.",
                    pad.id(),
                    pad.target_channel()
                );
                Some((pad.id(), pad.target_channel()))
            } else {
                None
            }
        })
    }
}
