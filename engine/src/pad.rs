use audioadapter::{Adapter, AdapterMut};
use audioadapter_buffers::direct::InterleavedSlice;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::sync::Arc;
use tracing::{debug, instrument, trace};
use triple_buffer::{Input, Output, TripleBuffer};

use crate::{Context, audio::Audio};

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
}

impl PadEngine {
    const MAX_VOICES: usize = 32;

    pub fn new(id: usize) -> (Self, HeapCons<f32>, Input<PadProperties>) {
        let (pad_in, pad_out) = HeapRb::<f32>::new(8192).split();
        let (prop_w, prop_r) = TripleBuffer::new(&PadProperties::new(id)).split();

        (
            Self {
                properties: prop_r,
                voices: Vec::with_capacity(Self::MAX_VOICES),
                producer: pad_in,
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
    pub fn process(&mut self, context: Context, buffer: &mut [f32]) -> bool {
        self.properties.update();
        let properties = self.properties.output_buffer();

        if self.voices.is_empty() {
            return false;
        }

        let audio = match properties.p_audio.as_ref() {
            Some(a) => {
                a
            }
            None => {
                return false;
            }
        };

        let channels = context.channels() as usize;
        let frames = buffer.len() / channels;

        let mut dst_adapter = InterleavedSlice::new_mut(buffer, channels, frames).unwrap();
        let src_adapter = audio.as_adapter();

        let vol = properties.volume;

        for voice in &mut self.voices {
            let remain_frames = src_adapter.frames().saturating_sub(voice.index);
            let process_frames = frames.min(remain_frames);

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
    context: Context,
    prealloc_render_buffer: Vec<f32>,
}

impl PadManager {
    pub fn new(context: Context) -> (Self, ringbuf::HeapProd<PadManagerCommand>) {
        let (cmd_tx, cmd_rx) = ringbuf::HeapRb::<PadManagerCommand>::new(25).split();
        (
            Self {
                pads: Vec::new(),
                cmd_rx,
                context,
                prealloc_render_buffer: vec![
                    0.0;
                    (context.buffer_size() as usize)
                        * (context.channels() as usize)
                        * 2
                ],
            },
            cmd_tx,
        )
    }

    fn run_cmd_queue(&mut self) {
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
    }

    /// Returns which pads are currently playing and where to route them to.
    /// impl Iterator is favored as return instead of Vec so there's no memory
    /// allocation during audio rendering, everything should be pre allocated
    /// beforehand.
    pub fn process(&mut self, buffer_size: usize) -> impl Iterator<Item = (usize, usize)> {
        self.run_cmd_queue();

        let buffer = &mut self.prealloc_render_buffer[..buffer_size];

        for p in &mut self.pads {
            buffer.fill(0.0);

            p.process(
                self.context,
                buffer,
            );
        }

        self.pads.iter().filter_map(|pad| {
            if pad.has_active_voices() {
                Some((pad.id(), pad.target_channel()))
            } else {
                None
            }
        })
    }
}
