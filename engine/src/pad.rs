use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Observer, Producer, Split},
};
use tracing::trace;
use std::sync::Arc;
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

pub(crate) struct PadEngine {
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

    pub fn has_active_voices(&self) -> bool {
        self.voices.is_empty()
    }

    pub fn process(&mut self) -> bool {
        self.properties.update();
        let properties = self.properties.output_buffer();

        if self.voices.is_empty() {
            return false
        }

        let audio = match properties.p_audio.as_ref() {
            Some(a) => a,
            None => {
                return false
            },
        };

        let available = self.producer.vacant_len();
        if available == 0 {
            return false;
        }

        let chunk_size = available.min(Self::MAX_CHUNK_SIZE);
        let buffer = &mut self.mix_buffer[..chunk_size];
        buffer.fill(0.0);

        let vol = properties.volume;

        for voice in &mut self.voices {
            let remain = audio.data.len().saturating_sub(voice.index);
            let frames = chunk_size.min(remain);

            let src = &audio.data[voice.index..voice.index + frames];
            let dst = &mut buffer[..frames];

            for (out, &sample) in dst.iter_mut().zip(src) {
                *out += sample * vol;
            }

            voice.index += frames;
        }

        let audio_len = audio.data.len();
        self.voices.retain(|v| v.index < audio_len);
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
    cmd_rx: crossbeam_channel::Receiver<PadManagerCommand>,
}

impl PadManager {
    pub fn new() -> (Self, crossbeam_channel::Sender<PadManagerCommand>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<PadManagerCommand>(100);
        (
            Self {
                pads: Vec::new(),
                cmd_rx,
            },
            cmd_tx,
        )
    }

    /// Returns which pads are currently playing and where to route them to.
    pub fn process(&mut self) -> impl Iterator<Item = (usize, usize)> {
        if let Ok(cmd) = self.cmd_rx.try_recv() { match cmd {
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
        } }

        for p in &mut self.pads {
            p.process();
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
