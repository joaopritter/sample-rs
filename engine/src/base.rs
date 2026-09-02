#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum BufferSize {
    S128 = 128,
    S256 = 256,
    S512 = 512,
    S1024 = 1024,
    S2048 = 2048,
    S4096 = 4096,
}

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum SampleRate {
    R44_1kHz = 44_100,
    R48kHz = 48_000,
}

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum Channels {
    Mono = 1,
    Stereo = 2,
}

#[derive(Debug, Clone, Copy)]
pub struct Context {
    sample_rate: SampleRate,
    buffer_size: BufferSize,
    channels: Channels,
}

impl Context {
    pub fn new(sample_rate: SampleRate, buffer_size: BufferSize, channels: Channels) -> Self {
        Self {
            sample_rate,
            buffer_size,
            channels,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    pub fn buffer_size(&self) -> u32 {
        self.buffer_size as u32
    }

    pub fn channels(&self) -> u32 {
        self.channels as u32
    }

    pub(crate) fn buffer_allocation_needed(&self) -> usize {
        (self.buffer_size as usize) * (self.channels as usize)
    }
}
