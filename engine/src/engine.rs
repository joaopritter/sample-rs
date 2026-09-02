use std::sync::Arc;

use ringbuf::{
    HeapProd, HeapRb, traits::{Consumer, Observer, Producer, Split},
};
use slab::Slab;
use tracing::{debug, info, info_span, instrument, warn};

use crate::{
    Context,
    audio::{Audio, decode_file},
    channel::{Channel, ChannelProperties},
    pad::{PadEngine, PadManager, PadManagerCommand, PadProperties},
    router::{AudioRouter, AudioRouterCommand},
};

pub struct OutputFeeder {
    consumer: ringbuf::HeapCons<f32>,
}

impl OutputFeeder {
    pub(crate) fn new(consumer: ringbuf::HeapCons<f32>) -> Self {
        Self { consumer }
    }

    pub fn feed(&mut self, buffer: &mut[f32]) {
        buffer.fill(0.0);
        self.consumer.pop_slice(buffer);
    }
}

struct InnerEngine {
    audio_router: AudioRouter,
    pad_router: PadManager,
    producer: HeapProd<f32>,
    scratch_buffer: Vec<f32>,
}

impl InnerEngine {
    fn new(
        context: Context,
        audio_router: AudioRouter,
        pad_router: PadManager,
    ) -> (Self, OutputFeeder) {
        let (producer, audio_out) = HeapRb::<f32>::new(context.buffer_allocation_needed()).split();
        (
            Self {
                audio_router,
                pad_router,
                producer,
                scratch_buffer: vec![0.0; context.buffer_allocation_needed()],
            },
            OutputFeeder::new(audio_out),
        )
    }

    fn inner_loop(&mut self) {
        let vacant = self.producer.vacant_len();

        if vacant == 0 {
            return;
        };

        let buffer = &mut self.scratch_buffer[..vacant];
        self.audio_router
            .process(self.pad_router.process(buffer.len()), buffer);
        self.producer.push_slice(buffer);
    }
}

pub struct Engine {
    context: Context,
    pad_rack: Slab<(PadProperties, triple_buffer::Input<PadProperties>)>,
    channel_rack: Slab<triple_buffer::Input<ChannelProperties>>,
    channel_tx: ringbuf::HeapProd<AudioRouterCommand>,
    pad_tx: ringbuf::HeapProd<PadManagerCommand>,
    audios: Slab<Arc<Audio>>,
}

impl Engine {
    // TODO: Engine return a Monitoring struct containing atomics displaying
    // the current memory usage, frame rendering, etc.
    pub fn start(context: Context) -> (Self, OutputFeeder) {
        let _span = info_span!("engine_start").entered();
        info!("Starting engine");
        info!(
            sample_rate = context.sample_rate(),
            buffer_size = context.buffer_size(),
            "Configured audio device"
        );
        let (audio_router, channel_tx) = AudioRouter::new(context);
        let (pad_manager, pad_tx) = PadManager::new(context);

        let (mut inner, audio_out) = InnerEngine::new(context, audio_router, pad_manager);

        std::thread::spawn(move || {
            loop {
                inner.inner_loop();
            }
        });

        (
            Self {
                pad_rack: Slab::new(),
                channel_rack: Slab::new(),
                channel_tx,
                pad_tx,
                audios: Slab::new(),
                context,
            },
            audio_out,
        )
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
