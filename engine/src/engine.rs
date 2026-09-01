use std::sync::Arc;

use cpal::{
    Device, StreamConfig,
    traits::{DeviceTrait, StreamTrait},
};
use ringbuf::traits::Producer;
use slab::Slab;
use tracing::{debug, error, info, info_span, instrument, warn};

use crate::{
    audio::{Audio, decode_file},
    channel::{Channel, ChannelProperties},
    pad::{PadEngine, PadManager, PadManagerCommand, PadProperties},
    router::{AudioRouter, AudioRouterCommand},
};

pub struct Engine {
    _stream: cpal::Stream,
    pad_rack: Slab<(PadProperties, triple_buffer::Input<PadProperties>)>,
    channel_rack: Slab<triple_buffer::Input<ChannelProperties>>,
    channel_tx: ringbuf::HeapProd<AudioRouterCommand>,
    pad_tx: ringbuf::HeapProd<PadManagerCommand>,
    stream_config: StreamConfig,
    audios: Slab<Arc<Audio>>,
}

impl Engine {
    pub fn start(device: Device) -> Self {
        let _span = info_span!("engine_start").entered();

        info!("Starting engine");
        let (mut audio_router, channel_tx) = AudioRouter::new();
        let (mut pad_manager, pad_tx) = PadManager::new();

        let stream_config = device.default_output_config().unwrap().config();
        info!(
            sample_rate = stream_config.sample_rate,
            "Configured audio device"
        );
        let channels = stream_config.channels;

        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _| {
                    audio_router.process(pad_manager.process(channels as usize), data);
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
            stream_config,
            audios: Slab::new(),
        }
    }

    pub fn load_file(&mut self, file: &std::path::Path) -> usize {
        let audio = decode_file(file).unwrap();
        let resampled = audio.resample(self.stream_config.sample_rate);
        self.audios.insert(Arc::new(resampled))
    }

    pub fn get_audio(&self, audio_id: usize) -> Arc<Audio> {
        let audio = self.audios.get(audio_id).unwrap();
        audio.clone()
    }

    #[instrument(skip(self))]
    pub fn add_channel(&mut self) -> usize {
        let entry = self.channel_rack.vacant_entry();
        let key = entry.key();
        info!("Attempting to create channel with id: {}", key);

        let (channel, channel_properties) = Channel::new(key);
        entry.insert(channel_properties);
        self.channel_tx
            .try_push(AudioRouterCommand::AddChannel(channel));

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
            .try_push(AudioRouterCommand::ConnectPad((key, pad_out)));
        self.pad_tx.try_push(PadManagerCommand::AddPad(pad));

        info!("Successfully created pad '{}'.", key);
        key
    }

    #[instrument(skip(self))]
    pub fn hit_pad(&mut self, pad_id: usize) {
        debug!("Sending hit command to pad manager");
        self.pad_tx.try_push(PadManagerCommand::Hit(pad_id));
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
}
