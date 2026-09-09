use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, ensure};
use hypr_onnx::{
    ndarray::Array3,
    ort::{self, session::Session, value::Tensor},
};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};

const RATE: usize = 16000;
const WINDOW: usize = RATE * 10;
const HOP: usize = RATE;
const FRAME: usize = 270;
const CENTER: usize = 495;
const POWERSET: [u8; 7] = [0, 1, 2, 4, 3, 5, 6];

#[derive(Debug)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_index: i32,
}

pub struct Diarizer {
    segmentation: Session,
    embedding: Session,
    features: Features,
}

impl Diarizer {
    pub fn load() -> Result<Self> {
        let path = super::models::Model::Diarizer.verify()?;
        Ok(Self {
            segmentation: hypr_onnx::load_model_from_path(path.join("model.onnx"))?,
            embedding: hypr_onnx::load_model_from_path(path.join("nemo_en_titanet_small.onnx"))?,
            features: Features::new(),
        })
    }

    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let features = self.features.compute(samples)?;
        let frames = features.shape()[2];
        let output = self.embedding.run(ort::inputs![
            "audio_signal" => Tensor::from_array(features)?,
            "length" => Tensor::from_array(([1], vec![frames as i64]))?,
        ])?;
        let (_, values) = output["embs"].try_extract_tensor::<f32>()?;
        let norm = values.iter().map(|x| x * x).sum::<f32>().sqrt();
        ensure!(norm.is_finite() && norm > 1e-8, "Invalid speaker embedding");
        Ok(values.iter().map(|v| v / norm).collect())
    }

    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Result<Vec<Segment>> {
        ensure!(
            sample_rate == RATE as u32,
            "Speaker detection requires 16 kHz mono audio"
        );
        ensure!(
            samples.iter().all(|s| s.is_finite()),
            "Invalid audio samples"
        );
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let mut embeddings = Vec::new();
        let mut windows = Vec::new();
        for offset in (0..samples.len()).step_by(HOP) {
            let mut audio = vec![0.0; WINDOW];
            let length = WINDOW.min(samples.len() - offset);
            audio[..length].copy_from_slice(&samples[offset..offset + length]);
            let labels = {
                let output = self
                    .segmentation
                    .run(ort::inputs![Tensor::from_array(([1, 1, WINDOW], audio))?])?;
                let (shape, values) = output[0].try_extract_tensor::<f32>()?;
                ensure!(
                    shape.len() == 3 && shape[2] == 7,
                    "Unexpected segmentation model output"
                );
                values
                    .chunks_exact(7)
                    .map(|row| {
                        let class = row
                            .iter()
                            .enumerate()
                            .max_by(|a, b| a.1.total_cmp(b.1))
                            .unwrap()
                            .0;
                        POWERSET[class]
                    })
                    .collect::<Vec<_>>()
            };
            let mut identities = [None; 3];
            for speaker in 0..3 {
                let collect = |exclude_overlap: bool| {
                    let mut selected = Vec::new();
                    for (frame, &mask) in labels.iter().enumerate() {
                        if mask & (1 << speaker) == 0 || (exclude_overlap && mask.count_ones() > 1)
                        {
                            continue;
                        }
                        let start = (offset + CENTER + frame * FRAME).min(samples.len());
                        let end = (start + FRAME).min(samples.len());
                        selected.extend_from_slice(&samples[start..end]);
                    }
                    selected
                };
                let mut selected = collect(true);
                if selected.len() < RATE / 2 {
                    selected = collect(false);
                }
                if selected.len() < RATE / 5 {
                    continue;
                }
                identities[speaker] = Some(embeddings.len());
                embeddings.push(self.embed(&selected)?);
            }
            windows.push((offset, labels, identities));
        }
        let assignments = cluster(&embeddings, 0.5);
        let frame_count = samples.len().div_ceil(FRAME);
        let mut votes = vec![BTreeMap::<usize, u16>::new(); frame_count];
        let mut counts = vec![(0_u16, 0_u16); frame_count];
        for (offset, labels, identities) in windows {
            for (local_frame, mask) in labels.into_iter().enumerate() {
                let frame = (offset + local_frame * FRAME + FRAME / 2) / FRAME;
                if frame >= frame_count {
                    break;
                }
                counts[frame].0 += mask.count_ones() as u16;
                counts[frame].1 += 1;
                for (speaker, identity) in identities.iter().enumerate() {
                    if mask & (1 << speaker) != 0 {
                        if let Some(identity) = identity {
                            *votes[frame].entry(assignments[*identity]).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut active = BTreeMap::new();
        let mut segments = Vec::new();
        for frame in 0..=frame_count {
            let mut speakers = Vec::new();
            if frame < frame_count {
                let (sum, count) = counts[frame];
                let count = if count == 0 {
                    0
                } else {
                    (sum + count / 2) / count
                };
                speakers.extend(votes[frame].iter().map(|(&speaker, &vote)| (speaker, vote)));
                speakers.sort_by_key(|&(speaker, vote)| (std::cmp::Reverse(vote), speaker));
                speakers.truncate(count as usize);
            }
            let time = (CENTER + frame * FRAME).min(samples.len()) as i64 * 1000 / RATE as i64;
            let stopped = active
                .keys()
                .copied()
                .filter(|id| !speakers.iter().any(|s| s.0 == *id))
                .collect::<Vec<_>>();
            for speaker in stopped {
                let start_ms = active.remove(&speaker).unwrap();
                if time - start_ms >= 100 {
                    segments.push(Segment {
                        start_ms,
                        end_ms: time,
                        speaker_index: speaker as i32,
                    });
                }
            }
            for (speaker, _) in speakers {
                active.entry(speaker).or_insert(time);
            }
        }
        segments.sort_by_key(|s| (s.start_ms, s.speaker_index));
        // Assign stable, consecutive labels in order of first appearance.
        let mut names = BTreeMap::new();
        for segment in &mut segments {
            let next = names.len() as i32;
            segment.speaker_index = *names.entry(segment.speaker_index).or_insert(next);
        }
        Ok(segments)
    }
}

// Complete-link clustering avoids merging distinct speakers through a chain of
// ambiguous short turns. Kodama uses a nearest-neighbor chain and condensed
// distances, so long meetings do not allocate a quadratic heap of pair objects.
fn cluster(embeddings: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    let n = embeddings.len();
    if n < 2 {
        return (0..n).collect();
    }
    let mut distances = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in i + 1..n {
            distances.push(
                (1.0 - embeddings[i]
                    .iter()
                    .zip(&embeddings[j])
                    .map(|(a, b)| a * b)
                    .sum::<f32>())
                .max(0.0),
            );
        }
    }
    let tree = kodama::linkage(&mut distances, n, kodama::Method::Complete);
    let mut parents: Vec<_> = (0..2 * n - 1).collect();
    for (i, step) in tree.steps().iter().enumerate() {
        if step.dissimilarity > threshold {
            break;
        }
        parents[step.cluster1] = n + i;
        parents[step.cluster2] = n + i;
    }
    (0..n)
        .map(|mut i| {
            while parents[i] != i {
                i = parents[i];
            }
            i
        })
        .collect()
}

struct Features {
    fft: Arc<dyn Fft<f32>>,
    filters: Vec<Vec<f32>>,
}

impl Features {
    fn new() -> Self {
        let max_mel = 15.0 + (8000.0_f64 / 1000.0).ln() / (6.4_f64.ln() / 27.0);
        let frequencies = (0..82)
            .map(|i| {
                let mel = max_mel * i as f64 / 81.0;
                if mel < 15.0 {
                    mel * 200.0 / 3.0
                } else {
                    1000.0 * ((mel - 15.0) * 6.4_f64.ln() / 27.0).exp()
                }
            })
            .collect::<Vec<_>>();
        let filters = frequencies
            .windows(3)
            .map(|f| {
                (0..257)
                    .map(|bin| {
                        let hz = bin as f64 * RATE as f64 / 512.0;
                        (((hz - f[0]) / (f[1] - f[0]))
                            .min((f[2] - hz) / (f[2] - f[1]))
                            .max(0.0)
                            * 2.0
                            / (f[2] - f[0])) as f32
                    })
                    .collect()
            })
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(512),
            filters,
        }
    }

    fn compute(&self, samples: &[f32]) -> Result<Array3<f32>> {
        ensure!(samples.len() >= 400, "Speaker sample too short");
        let frames = (samples.len() - 400) / 160 + 1;
        let mut features = Array3::<f32>::zeros((1, 80, frames));
        let mut spectrum = vec![Complex32::default(); 512];
        for frame in 0..frames {
            spectrum.fill(Complex32::default());
            let offset = frame * 160;
            for i in 0..400 {
                let value = samples[offset + i] - 0.97 * samples[offset + i.saturating_sub(1)];
                spectrum[i].re =
                    value * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / 400.0).cos());
            }
            self.fft.process(&mut spectrum);
            for (mel, filter) in self.filters.iter().enumerate() {
                let energy = spectrum
                    .iter()
                    .zip(filter)
                    .map(|(v, weight)| v.norm_sqr() * weight)
                    .sum::<f32>();
                features[[0, mel, frame]] = energy.max(f32::EPSILON).ln();
            }
        }
        for mel in 0..80 {
            let mean = (0..frames).map(|t| features[[0, mel, t]]).sum::<f32>() / frames as f32;
            let variance = (0..frames)
                .map(|t| (features[[0, mel, t]] - mean).powi(2))
                .sum::<f32>()
                / frames as f32;
            let scale = variance.sqrt() + 1e-5;
            for frame in 0..frames {
                features[[0, mel, frame]] = (features[[0, mel, frame]] - mean) / scale;
            }
        }
        ensure!(
            features.iter().all(|v| v.is_finite()),
            "Invalid speaker features"
        );
        Ok(features)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the public six-speaker fixture and downloaded speaker models"]
    fn embedding_matches_reference() {
        let path =
            std::env::var("LOOFAH_DIARIZATION_FIXTURE").expect("Set LOOFAH_DIARIZATION_FIXTURE");
        let reader = hound::WavReader::open(path).unwrap();
        let samples = reader
            .into_samples::<i16>()
            .take(RATE * 3)
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect::<Vec<_>>();
        // Independent Kaldi-native-fbank preprocessing and ONNX Runtime inference
        // on the first three seconds of pyannote-rs v0.1.0/6_speakers.wav.
        let expected: Vec<f32> =
            serde_json::from_str(include_str!("../tests/data/titanet-reference.json")).unwrap();
        let actual = Diarizer::load().unwrap().embed(&samples).unwrap();
        let norm = expected.iter().map(|v| v * v).sum::<f32>().sqrt();
        let similarity = actual
            .iter()
            .zip(expected)
            .map(|(a, b)| a * b / norm)
            .sum::<f32>();
        assert!(
            similarity > 0.999,
            "Embedding reference cosine: {similarity}"
        );
    }

    #[test]
    fn clusters_more_than_four_speakers_and_keeps_repeated_turns_together() {
        let embeddings = (0..12)
            .map(|i| (0..6).map(|j| if j == i % 6 { 1.0 } else { 0.0 }).collect())
            .collect::<Vec<Vec<f32>>>();
        let labels = cluster(&embeddings, 0.35);
        assert_eq!(
            labels
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            6
        );
        for i in 0..6 {
            assert_eq!(labels[i], labels[i + 6]);
        }
    }

    #[test]
    fn silence_features_are_finite() {
        assert!(
            Features::new()
                .compute(&vec![0.0; RATE])
                .unwrap()
                .iter()
                .all(|f| f.is_finite())
        );
        assert!(cluster(&[], 0.35).is_empty());
    }
}
