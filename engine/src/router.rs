use ringbuf::{
    HeapCons,
    traits::{Consumer, Split},
};
use tracing::trace;

use crate::{Context, channel::Channel};

pub enum AudioRouterCommand {
    ConnectPad((usize, HeapCons<f32>)),
    DisconnectPad(usize),
    AddChannel(Channel),
    RemoveChannel(usize),
}

pub struct AudioRouter {
    /// Pad.id and consumer
    pads: Vec<(usize, HeapCons<f32>)>,
    channels: Vec<Channel>,
    /// Pre-allocated memory forwarded to each channel to use
    /// during processing.
    prealloc_mix_buffer: Vec<f32>,
    prealloc_active_routings: Vec<(usize, usize)>,
    cmd_rx: ringbuf::HeapCons<AudioRouterCommand>,
    context: Context,
}

impl AudioRouter {
    const MAX_PADS: usize = 64;
    pub fn new(context: Context) -> (Self, ringbuf::HeapProd<AudioRouterCommand>) {
        let (cmd_tx, cmd_rx) = ringbuf::HeapRb::<AudioRouterCommand>::new(25).split();
        (
            Self {
                pads: Vec::with_capacity(Self::MAX_PADS),
                channels: Vec::with_capacity(16),
                prealloc_mix_buffer: vec![0.0; context.buffer_allocation_needed()],
                prealloc_active_routings: Vec::with_capacity(Self::MAX_PADS),
                cmd_rx,
                context,
            },
            cmd_tx,
        )
    }

    fn run_cmd_queue(&mut self) {
        while let Some(cmd) = self.cmd_rx.try_pop() {
            match cmd {
                AudioRouterCommand::ConnectPad(pad) => {
                    trace!("Connecting pad '{}' to audio router.", pad.0);
                    self.pads.push(pad);
                }
                AudioRouterCommand::AddChannel(channel) => {
                    trace!("Pushing channel '{}' to vector...", channel.id());
                    self.channels.push(channel);
                }
                AudioRouterCommand::DisconnectPad(pad_id) => todo!(),
                AudioRouterCommand::RemoveChannel(channel_id) => todo!(),
            }
        }
    }

    /// Routes the buffer of each pad to its channel
    pub fn process(
        &mut self,
        active_pads: impl Iterator<Item = (usize, usize)>,
        output: &mut [f32],
    ) {
        self.run_cmd_queue();

        self.prealloc_active_routings.clear();
        for routing in active_pads {
            self.prealloc_active_routings.push(routing);
        }

        output.fill(0.0);
        let buffer_size = output.len();

        assert!(
            self.prealloc_mix_buffer.capacity() >= buffer_size,
            "Audio buffer size exceeded pre-allocated capacity!"
        );

        let mix_buffer = &mut self.prealloc_mix_buffer[..buffer_size];

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
