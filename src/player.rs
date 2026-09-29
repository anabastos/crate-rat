//! Audio decoding for playback. rodio's own symphonia wrapper (0.19) never tells symphonia the
//! file's size, which makes it panic on M4A files whose index sits at the end of the file (the
//! usual layout for tidal-dl-ng downloads) — taking the whole app down. This wraps symphonia
//! directly around a real `File` (which does report its size) and exposes it as a rodio
//! `Source`.

use std::{fs::File, path::Path, time::Duration};

use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions, CODEC_TYPE_NULL},
    errors::Error as SymphoniaError,
    formats::{FormatOptions, FormatReader},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    buffer: Vec<f32>,
    position: usize,
    channels: u16,
    sample_rate: u32,
    total_duration: Option<Duration>,
}

impl SymphoniaSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| format!("Could not open that track: {error}"))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(&hint, stream, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
            .map_err(|error| format!("Unsupported or unreadable audio format: {error}"))?;
        let format = probed.format;
        let track = format.tracks().iter().find(|track| track.codec_params.codec != CODEC_TYPE_NULL).ok_or("No playable audio track in that file")?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|error| format!("Unsupported codec: {error}"))?;
        let total_duration = params.time_base.zip(params.n_frames).map(|(time_base, frames)| {
            let time = time_base.calc_time(frames);
            Duration::from_secs(time.seconds) + Duration::from_secs_f64(time.frac)
        });
        let mut source = SymphoniaSource {
            format,
            decoder,
            track_id,
            buffer: Vec::new(),
            position: 0,
            channels: params.channels.map_or(2, |channels| channels.count() as u16),
            sample_rate: params.sample_rate.unwrap_or(44_100),
            total_duration,
        };
        // Decode the first packet up front so a broken file fails here (with a message)
        // rather than as silence, and so channels/sample rate come from real decoded audio.
        if !source.decode_next() {
            return Err("That file has no decodable audio".into());
        }
        Ok(source)
    }

    /// Refills `buffer` with the next packet's samples; false at end of stream.
    fn decode_next(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                // End of file, or a stream that needs a reset (chained Ogg etc.) — stop there.
                Err(_) => return false,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if decoded.frames() == 0 {
                        continue;
                    }
                    let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    samples.copy_interleaved_ref(decoded);
                    self.channels = spec.channels.count() as u16;
                    self.sample_rate = spec.rate;
                    self.buffer.clear();
                    self.buffer.extend_from_slice(samples.samples());
                    self.position = 0;
                    return true;
                }
                // A corrupt packet: skip it and keep playing.
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => return false,
            }
        }
    }
}

impl Iterator for SymphoniaSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.position >= self.buffer.len() && !self.decode_next() {
            return None;
        }
        let sample = self.buffer[self.position];
        self.position += 1;
        Some(sample)
    }
}

impl rodio::Source for SymphoniaSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Channel count / sample rate may only change at a packet boundary.
        Some(self.buffer.len() - self.position)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
}
