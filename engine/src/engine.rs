use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::traits::Producer;
use slab::Slab;
use tracing::{debug, error, info, info_span, instrument, warn};

use crate::{
    Context,
    audio::{Audio, decode_file},
    channel::{Channel, ChannelProperties},
    pad::{PadEngine, PadManager, PadManagerCommand, PadProperties},
    router::{AudioRouter, AudioRouterCommand},
};

pub struct EngineSettings {
    device: cpal::Device,
    context: Context,
}

impl EngineSettings {
    pub fn new(device: cpal::Device, context: Context) -> Self {
        Self { device, context }
    }
}

pub struct Engine {
    _stream: cpal::Stream,
    device: cpal::Device,
    context: Context,
    pad_rack: Slab<(PadProperties, triple_buffer::Input<PadProperties>)>,
    channel_rack: Slab<triple_buffer::Input<ChannelProperties>>,
    channel_tx: ringbuf::HeapProd<AudioRouterCommand>,
    pad_tx: ringbuf::HeapProd<PadManagerCommand>,
    audios: Slab<Arc<Audio>>,
}

impl Engine {
    pub fn start(settings: EngineSettings) -> Self {
        let _span = info_span!("engine_start").entered();

        let context = settings.context;
        let device = settings.device;

        let desired_buffer_size = context.buffer_size();
        let supported_config = device.default_output_config().unwrap();

        match supported_config.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => {
                if *min > desired_buffer_size || *max < desired_buffer_size {
                    panic!()
                }
            }
            cpal::SupportedBufferSize::Unknown => todo!(),
        }

        let mut stream_config = supported_config.config();
        stream_config.buffer_size = cpal::BufferSize::Fixed(desired_buffer_size);
        stream_config.sample_rate = context.sample_rate();

        info!("Starting engine");
        info!(
            sample_rate = stream_config.sample_rate,
            buffer_size = ?stream_config.buffer_size,
            "Configured audio device"
        );
        let (mut audio_router, channel_tx) = AudioRouter::new(context);
        let (mut pad_manager, pad_tx) = PadManager::new(context);

        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _| {
                    audio_router.process(pad_manager.process(data.len()), data);
                },
                |err| error!(%err, "Audio stream error occurred"),
                None,
            )
            .unwrap();

        stream.play().unwrap();

        Self {
            _stream: stream,
            device,
            pad_rack: Slab::new(),
            channel_rack: Slab::new(),
            channel_tx,
            pad_tx,
            audios: Slab::new(),
            context,
        }
    }

    pub fn load_file(&mut self, file: &std::path::Path) -> usize {
        let audio = decode_file(file).unwrap();
        let resampled = audio.resample(self.context.sample_rate());
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
