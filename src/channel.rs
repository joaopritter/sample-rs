use ringbuf::{HeapCons, traits::Consumer};
use triple_buffer::{Input, Output, TripleBuffer};

#[derive(Debug, Clone)]
pub struct ChannelProperties {
    id: usize,
    volume: f32,
}

impl ChannelProperties {
    pub fn new(id: usize) -> Self {
        Self { id, ..Default::default() }
    }

}

impl Default for ChannelProperties {
    fn default() -> Self {
        Self {
            id: 0,
            volume: 1.0,
        }
    }
}

pub struct Channel {
    properties: Output<ChannelProperties>,
    mix_buffer: Vec<f32>,
}

impl Channel {
    pub fn new(id: usize) -> (Self, Input<ChannelProperties>) {
        let (prop_w, prop_r) = TripleBuffer::new(&ChannelProperties::new(id)).split();
        (
            Self {
                mix_buffer: Vec::new(),
                properties: prop_r,
            },
            prop_w,
        )
    }

    pub fn id(&self) -> usize {
        self.properties.output_buffer().id
    }

    pub fn process<'a>(
        &mut self,
        inputs: impl Iterator<Item = &'a mut HeapCons<f32>>,
        buffer: &mut [f32],
    ) {
        let frames = buffer.len();

        if self.mix_buffer.len() < frames {
            self.mix_buffer.resize(frames, 0.0);
        }

        self.mix_buffer[..frames].fill(0.0);

        for input in inputs {
            for (mix_sample, in_sample) in
                self.mix_buffer[..frames].iter_mut().zip(input.pop_iter())
            {
                *mix_sample += in_sample;
            }
        }

        let volume = self.properties.output_buffer().volume;

        for (out_sample, &mix_sample) in buffer.iter_mut().zip(self.mix_buffer[..frames].iter()) {
            *out_sample = mix_sample * volume;
        }
    }
}
