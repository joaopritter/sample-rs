use std::sync::Arc;

use cpal::Device;
use cpal::traits::{DeviceTrait, StreamTrait};
use sample_rs_sound::Audio;
use slab::Slab;
use tracing::{debug, error, info, info_span, instrument, warn};
use triple_buffer::Input;

use crate::channel::{Channel, ChannelProperties};
use crate::pad::{PadEngine, PadManager, PadManagerCommand, PadProperties};
use crate::router::{AudioRouter, AudioRouterCommand};

pub struct Engine {
    _stream: cpal::Stream,
    pad_rack: Slab<(PadProperties, Input<PadProperties>)>,
    channel_rack: Slab<Input<ChannelProperties>>,
    channel_tx: crossbeam_channel::Sender<AudioRouterCommand>,
    pad_tx: crossbeam_channel::Sender<PadManagerCommand>,
}

impl Engine {
    pub fn start(device: Device) -> Self {
        let _span = info_span!("engine_start").entered();

        info!("Starting engine");
        let (mut audio_router, channel_tx) = AudioRouter::new();
        let (mut pad_manager, pad_tx) = PadManager::new();

        let config = device.default_output_config().unwrap();
        info!(
            sample_rate = config.sample_rate(),
            "Configured audio device"
        );

        let stream = device
            .build_output_stream(
                config.into(),
                move |data: &mut [f32], _| {
                    audio_router.process(pad_manager.process(), data);
                },
                |err| error!(%err, "Audio stream error occurred"),
                None,
            )
            .unwrap();

        stream.play().unwrap();

        Self {
            _stream: stream,
            pad_rack: Slab::new(),
            channel_rack: Slab::new(),
            channel_tx,
            pad_tx,
        }
    }

    #[instrument(skip(self))]
    pub fn add_channel(&mut self) -> usize {
        let entry = self.channel_rack.vacant_entry();
        let key = entry.key();
        info!("Attempting to create channel with id: {}", key);

        let (channel, channel_properties) = Channel::new(key);
        entry.insert(channel_properties);
        self.channel_tx
            .send(AudioRouterCommand::AddChannel(channel));

        info!("Successfully created channel '{}'.", key);
        key
    }

    #[instrument(skip(self))]
    pub fn add_pad(&mut self) -> usize {
        let entry = self.pad_rack.vacant_entry();
        let key = entry.key();
        info!("Attempting to create pad with id: {}", key);

        let master_properties = PadProperties::new(key);
        let (pad, pad_out, pad_buffer_in) = PadEngine::new(key);

        entry.insert((master_properties, pad_buffer_in));
        self.channel_tx
            .send(AudioRouterCommand::ConnectPad((key, pad_out)));
        self.pad_tx.send(PadManagerCommand::AddPad(pad));

        info!("Successfully created pad '{}'.", key);
        key
    }

    #[instrument(skip(self))]
    pub fn hit_pad(&self, pad_id: usize) {
        debug!("Sending hit command to pad manager");
        self.pad_tx.send(PadManagerCommand::Hit(pad_id));
    }

    #[instrument(skip(self, update_fn))]
    pub fn update_pad<F>(&mut self, pad_id: usize, mut update_fn: F)
    where
        F: FnMut(&mut PadProperties),
    {
        if let Some((master, buffer_in)) = self.pad_rack.get_mut(pad_id) {
            update_fn(master);
            *buffer_in.input_buffer_mut() = master.clone();
            buffer_in.publish();
        } else {
            warn!("Couldn't find pad with id '{}' to update.", pad_id);
        }
    }

    #[instrument(skip(self))]
    pub fn route_pad_to_channel(&mut self, pad_id: usize, channel_id: usize) {
        self.update_pad(pad_id, |props| {
            debug!("Routing pad '{}' to channel '{}'.", pad_id, channel_id);
            props.set_target_channel(channel_id);
        });
    }

    #[instrument(skip(self, audio))]
    pub fn load_audio(&mut self, pad_id: usize, audio: Option<Arc<Audio>>) {
        info!("Loading audio to pad '{}'.", pad_id);
        self.update_pad(pad_id, |props| {
            props.set_audio(audio.clone());
        });
    }
}
