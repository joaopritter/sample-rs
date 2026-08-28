use ringbuf::HeapCons;
use tracing::trace;

use crate::channel::Channel;

pub enum AudioRouterCommand {
    ConnectPad((usize, HeapCons<f32>)),
    AddChannel(Channel),
}

pub struct AudioRouter {
    pads: Vec<(usize, HeapCons<f32>)>,
    channels: Vec<Channel>,
    /// Pre-allocated memory forwarded to each channel to use
    /// during processing.
    prealloc_mix_buffer: Vec<f32>,
    prealloc_active_routings: Vec<(usize, usize)>,
    cmd_rx: crossbeam_channel::Receiver<AudioRouterCommand>,
}

impl AudioRouter {
    const MAX_CHUNK_SIZE: usize = 1024;
    const MAX_PADS: usize = 64;
    pub fn new() -> (Self, crossbeam_channel::Sender<AudioRouterCommand>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioRouterCommand>(100);
        (
            Self {
                pads: Vec::with_capacity(Self::MAX_PADS),
                channels: Vec::with_capacity(16),
                prealloc_mix_buffer: vec![0.0; Self::MAX_CHUNK_SIZE],
                prealloc_active_routings: Vec::with_capacity(Self::MAX_PADS),
                cmd_rx,
            },
            cmd_tx,
        )
    }

    pub fn connect_pad(&mut self, pad: (usize, HeapCons<f32>)) {
        trace!("Connecting pad '{}' to audio router.", pad.0);
        self.pads.push(pad);
    }

    pub fn new_channel(&mut self, channel: Channel) {
        trace!("Pushing channel '{}' to vector...", channel.id());
        self.channels.push(channel);
    }

    /// Routes the buffer of each pad to its channel
    pub fn process(
        &mut self,
        active_pads: impl Iterator<Item = (usize, usize)>,
        output: &mut [f32],
    ) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                AudioRouterCommand::ConnectPad(pad) => self.connect_pad(pad),
                AudioRouterCommand::AddChannel(channel) => self.new_channel(channel),
            }
        }

        let frames = output.len();
        if frames == 0 {
            return;
        }

        self.prealloc_active_routings.clear();
        for routing in active_pads {
            if self.prealloc_active_routings.len() < self.prealloc_active_routings.capacity() {
                self.prealloc_active_routings.push(routing);
            }
        }

        output.fill(0.0);

        let chunk_size = frames.min(Self::MAX_CHUNK_SIZE);
        let mix_buffer = &mut self.prealloc_mix_buffer[..chunk_size];

        for channel in self.channels.iter_mut() {
            let channel_id = channel.id();
            mix_buffer.fill(0.0);

            let inputs = self.pads.iter_mut().filter_map(|(pad_id, pad_out)| {
                let is_routed_here = self
                    .prealloc_active_routings
                    .iter()
                    .any(|&(id, target_chan)| id == *pad_id && target_chan == channel_id);

                if is_routed_here { Some(pad_out) } else { None }
            });

            channel.process(inputs, mix_buffer);

            for (out_sample, &channel_sample) in output.iter_mut().zip(mix_buffer.iter()) {
                *out_sample += channel_sample;
            }
        }
    }
}
