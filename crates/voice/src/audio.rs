//! Streaming mono conversion and anti-aliasing resampling, independent of the input callback size.
#![cfg_attr(not(windows), allow(dead_code))]

pub const RATE: usize = 16_000;

pub struct Resampler {
    input_rate: u32,
    channels: usize,
    frame: Vec<f32>,
    position: f64,
    next: f64,
    previous: f32,
    filter: [f32; 4],
    alpha: f32,
}
impl Resampler {
    pub fn new(input_rate: u32, channels: usize) -> Self {
        let cutoff = (input_rate as f32 * 0.4).min(6_000.0);
        let alpha = 1.0 - (-std::f32::consts::TAU * cutoff / input_rate.max(1) as f32).exp();
        Self {
            input_rate: input_rate.max(1),
            channels: channels.max(1),
            frame: Vec::with_capacity(channels),
            position: 0.0,
            next: 0.0,
            previous: 0.0,
            filter: [0.0; 4],
            alpha,
        }
    }
    pub fn push(&mut self, input: impl Iterator<Item = f32>, out: &mut Vec<f32>) {
        for sample in input {
            self.frame.push(if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            });
            if self.frame.len() != self.channels {
                continue;
            }
            let mut mono = self.frame.iter().sum::<f32>() / self.channels as f32;
            self.frame.clear();
            if self.input_rate > RATE as u32 {
                for stage in &mut self.filter {
                    *stage += self.alpha * (mono - *stage);
                    mono = *stage;
                }
            }
            while self.next <= self.position {
                let fraction = (self.next - (self.position - 1.0)).clamp(0.0, 1.0) as f32;
                out.push(self.previous + (mono - self.previous) * fraction);
                self.next += f64::from(self.input_rate) / RATE as f64;
            }
            self.previous = mono;
            self.position += 1.0;
        }
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32)
        .sqrt()
        .clamp(0.0, 1.0)
}

/// An utterance ends on silence or a processing boundary, not a session duration cap.
pub struct Utterance {
    pub samples: Vec<f32>,
    silence: usize,
    speech: bool,
}
impl Utterance {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            silence: 0,
            speech: false,
        }
    }
    pub fn push(&mut self, samples: &[f32]) -> bool {
        let speech = rms(samples) >= 0.008;
        if speech {
            self.speech = true;
            self.silence = 0;
        } else {
            self.silence += samples.len();
        }
        self.samples.extend_from_slice(samples);
        if !self.speech && self.samples.len() > RATE / 4 {
            self.samples.drain(..self.samples.len() - RATE / 4);
        }
        self.speech && (self.silence >= RATE * 3 / 4 || self.samples.len() >= RATE * 12)
    }
    pub fn take(&mut self) -> Vec<f32> {
        self.silence = 0;
        self.speech = false;
        std::mem::take(&mut self.samples)
    }
    pub fn has_speech(&self) -> bool {
        self.speech
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stereo_and_callback_splits_are_equivalent() {
        let input: Vec<f32> = (0..96_000)
            .map(|i| ((i / 2) as f32 * 0.1).sin() * 0.2)
            .collect();
        let mut whole = Vec::new();
        Resampler::new(48_000, 2).push(input.iter().copied(), &mut whole);
        let mut split = Vec::new();
        let mut resampler = Resampler::new(48_000, 2);
        for chunk in input.chunks(137) {
            resampler.push(chunk.iter().copied(), &mut split);
        }
        assert_eq!(whole, split);
        assert_eq!(whole.len(), RATE);
        assert!(rms(&whole) > 0.01);
    }
    #[test]
    fn silence_does_not_create_unbounded_audio_or_utterances() {
        let mut utterance = Utterance::new();
        for _ in 0..1000 {
            assert!(!utterance.push(&[0.0; 1600]));
        }
        assert!(!utterance.has_speech());
        assert_eq!(utterance.samples.len(), RATE / 4);
    }
    #[test]
    fn speech_flushes_at_silence_and_processing_boundary() {
        let mut utterance = Utterance::new();
        assert!(!utterance.push(&[0.1; 1600]));
        assert!(utterance.push(&vec![0.0; RATE]));
        assert!(!utterance.take().is_empty());
        assert!(!utterance.has_speech());
        assert!(utterance.push(&vec![0.1; RATE * 12]));
    }
    #[test]
    fn nonfinite_audio_is_sanitized_and_upsampling_is_supported() {
        let mut out = Vec::new();
        Resampler::new(8_000, 1).push([f32::NAN, f32::INFINITY, 0.5, -2.0].into_iter(), &mut out);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert_eq!(out.len(), 7);
        assert_eq!(rms(&[]), 0.0);
    }
}
