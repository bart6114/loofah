use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{Stream, pin_mut};
use hypr_audio_interface::AsyncSource;
use pin_project::pin_project;

use crate::{Async, FixedAsync, PolynomialDegree, RubatoChunkResampler};

pub trait ResampleExtDynamicNew: AsyncSource + Sized {
    fn resampled_chunks(
        self,
        target_rate: u32,
        output_chunk_size: usize,
    ) -> Result<ResamplerDynamicNew<Self>, crate::Error> {
        ResamplerDynamicNew::new(self, target_rate, output_chunk_size)
    }
}

impl<T> ResampleExtDynamicNew for T where T: AsyncSource + Sized {}

enum Backend {
    Passthrough(Vec<f32>),
    Resampler(Box<RubatoChunkResampler<Async<f32>, 1>>),
}

impl Backend {
    fn passthrough(capacity: usize) -> Self {
        Self::Passthrough(Vec::with_capacity(capacity))
    }

    fn ensure_passthrough(&mut self, capacity: usize) {
        match self {
            Self::Passthrough(buffer) => buffer.clear(),
            Self::Resampler(_) => *self = Self::passthrough(capacity),
        }
    }

    fn ensure_resampler(
        &mut self,
        resampler: Async<f32>,
        output_chunk_size: usize,
        input_block_size: usize,
    ) {
        match self {
            Self::Passthrough(_) => {
                *self = Self::Resampler(Box::new(RubatoChunkResampler::new(
                    resampler,
                    output_chunk_size,
                    input_block_size,
                )));
            }
            Self::Resampler(driver) => {
                driver.rebind_resampler(resampler, output_chunk_size, input_block_size)
            }
        }
    }

    fn push_sample(&mut self, sample: f32) {
        match self {
            Self::Passthrough(buffer) => buffer.push(sample),
            Self::Resampler(driver) => driver.push_sample(sample),
        }
    }

    fn try_yield_chunk(&mut self, chunk_size: usize, allow_partial: bool) -> Option<Vec<f32>> {
        match self {
            Self::Passthrough(buffer) => {
                if buffer.len() >= chunk_size {
                    Some(buffer.drain(..chunk_size).collect())
                } else if allow_partial && !buffer.is_empty() {
                    Some(std::mem::take(buffer))
                } else {
                    None
                }
            }
            Self::Resampler(driver) => {
                if driver.has_full_chunk() {
                    driver.take_full_chunk()
                } else if allow_partial && !driver.output_is_empty() {
                    driver.take_all_output()
                } else {
                    None
                }
            }
        }
    }

    fn process_all_ready_blocks(&mut self) -> Result<bool, crate::Error> {
        match self {
            Self::Passthrough(_) => Ok(false),
            Self::Resampler(driver) => Ok(driver.process_all_ready_blocks()?),
        }
    }

    fn drain_for_rate_change(&mut self) -> Result<bool, crate::Error> {
        match self {
            Self::Passthrough(buffer) => Ok(buffer.is_empty()),
            Self::Resampler(driver) => {
                driver.process_all_ready_blocks()?;
                if driver.has_input() {
                    driver.process_partial_block(true)?;
                }
                Ok(driver.output_is_empty())
            }
        }
    }

    fn drain_at_eos(&mut self) -> Result<(), crate::Error> {
        match self {
            Self::Passthrough(_) => Ok(()),
            Self::Resampler(driver) => {
                driver.process_all_ready_blocks()?;
                if driver.has_input() {
                    driver.process_partial_block(true)?;
                }
                Ok(())
            }
        }
    }
}

#[pin_project]
pub struct ResamplerDynamicNew<S>
where
    S: AsyncSource,
{
    source: S,
    target_rate: u32,
    output_chunk_size: usize,
    input_block_size: usize,
    backend: Backend,
    last_source_rate: u32,
    draining: bool,
    pending_sample: Option<(f32, u32)>,
}

impl<S> ResamplerDynamicNew<S>
where
    S: AsyncSource,
{
    pub fn new(
        mut source: S,
        target_rate: u32,
        output_chunk_size: usize,
    ) -> Result<Self, crate::Error> {
        let source_rate = source.sample_rate();
        let input_block_size = output_chunk_size;
        let backend = if source_rate == target_rate {
            Backend::passthrough(output_chunk_size)
        } else {
            let ratio = target_rate as f64 / source_rate as f64;
            let resampler = match Self::create_resampler(ratio, input_block_size) {
                Ok(resampler) => resampler,
                Err(error) => {
                    source.request_stop();
                    return Err(error);
                }
            };
            Backend::Resampler(Box::new(RubatoChunkResampler::new(
                resampler,
                output_chunk_size,
                input_block_size,
            )))
        };
        Ok(Self {
            source,
            target_rate,
            output_chunk_size,
            input_block_size,
            backend,
            last_source_rate: source_rate,
            draining: false,
            pending_sample: None,
        })
    }

    pub fn request_stop_source(&mut self) -> bool {
        self.source.request_stop()
    }

    fn rebuild_backend(&mut self, new_rate: u32) -> Result<(), crate::Error> {
        if new_rate == self.target_rate {
            self.backend.ensure_passthrough(self.output_chunk_size);
        } else {
            let ratio = self.target_rate as f64 / new_rate as f64;
            let resampler = Self::create_resampler(ratio, self.input_block_size)?;
            self.backend
                .ensure_resampler(resampler, self.output_chunk_size, self.input_block_size);
        }
        self.last_source_rate = new_rate;
        Ok(())
    }

    fn try_yield_chunk(&mut self, allow_partial: bool) -> Option<Vec<f32>> {
        self.backend
            .try_yield_chunk(self.output_chunk_size, allow_partial)
    }

    fn drain_for_rate_change(&mut self) -> Result<bool, crate::Error> {
        self.backend.drain_for_rate_change()
    }

    fn drain_at_eos(&mut self) -> Result<(), crate::Error> {
        self.backend.drain_at_eos()
    }

    fn create_resampler(ratio: f64, input_block_size: usize) -> Result<Async<f32>, crate::Error> {
        Async::<f32>::new_poly(
            ratio,
            2.0,
            PolynomialDegree::Cubic,
            input_block_size.max(1),
            1,
            FixedAsync::Input,
        )
        .map_err(Into::into)
    }
}

impl<S> Stream for ResamplerDynamicNew<S>
where
    S: AsyncSource,
{
    type Item = Result<Vec<f32>, crate::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();

        loop {
            if let Some((sample, new_rate)) = me.pending_sample.take() {
                match me.drain_for_rate_change() {
                    Ok(true) => {
                        if let Err(err) = me.rebuild_backend(new_rate) {
                            return Poll::Ready(Some(Err(err)));
                        }
                        me.backend.push_sample(sample);
                        continue;
                    }
                    Ok(false) => {
                        if let Some(chunk) = me.try_yield_chunk(true) {
                            me.pending_sample = Some((sample, new_rate));
                            return Poll::Ready(Some(Ok(chunk)));
                        }
                        me.pending_sample = Some((sample, new_rate));
                        continue;
                    }
                    Err(err) => return Poll::Ready(Some(Err(err))),
                }
            }

            if let Some(chunk) = me.try_yield_chunk(me.draining) {
                return Poll::Ready(Some(Ok(chunk)));
            }

            if me.draining {
                return Poll::Ready(None);
            }

            match me.backend.process_all_ready_blocks() {
                Ok(true) => continue,
                Ok(false) => {}
                Err(err) => return Poll::Ready(Some(Err(err))),
            }

            let sample_poll = {
                let inner = me.source.as_stream();
                pin_mut!(inner);
                inner.poll_next(cx)
            };

            let sample = match sample_poll {
                Poll::Ready(Some(sample)) => sample,
                Poll::Ready(None) => {
                    if let Err(err) = me.drain_at_eos() {
                        return Poll::Ready(Some(Err(err)));
                    }
                    me.draining = true;
                    continue;
                }
                Poll::Pending => return Poll::Pending,
            };

            let current_rate = me.source.sample_rate();
            if current_rate != me.last_source_rate {
                me.pending_sample = Some((sample, current_rate));
                continue;
            }

            me.backend.push_sample(sample);
        }
    }
}

#[cfg(test)]
mod drain_tests {
    use super::*;
    use std::collections::VecDeque;
    struct Source {
        samples: VecDeque<f32>,
        rate: u32,
        stopped: bool,
    }
    impl AsyncSource for Source {
        fn as_stream(&mut self) -> impl Stream<Item = f32> + '_ {
            futures_util::stream::poll_fn(move |_| match self.samples.pop_front() {
                Some(sample) => Poll::Ready(Some(sample)),
                None if self.stopped => Poll::Ready(None),
                None => Poll::Pending,
            })
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
        fn request_stop(&mut self) -> bool {
            self.stopped = true;
            true
        }
    }

    #[test]
    fn failed_construction_requests_source_cleanup_before_drop() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct Acquired {
            stopped: Arc<AtomicBool>,
            cleanup_observed: Arc<AtomicBool>,
        }
        impl AsyncSource for Acquired {
            fn as_stream(&mut self) -> impl Stream<Item = f32> + '_ {
                futures_util::stream::pending()
            }
            fn sample_rate(&self) -> u32 {
                16_000
            }
            fn request_stop(&mut self) -> bool {
                self.stopped.store(true, Ordering::Release);
                true
            }
        }
        impl Drop for Acquired {
            fn drop(&mut self) {
                self.cleanup_observed
                    .store(self.stopped.load(Ordering::Acquire), Ordering::Release);
            }
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let cleanup_observed = Arc::new(AtomicBool::new(false));
        assert!(
            ResamplerDynamicNew::new(
                Acquired {
                    stopped: stopped.clone(),
                    cleanup_observed: cleanup_observed.clone(),
                },
                0,
                1_920
            )
            .is_err()
        );
        assert!(stopped.load(Ordering::Acquire));
        assert!(cleanup_observed.load(Ordering::Acquire));
    }
    #[test]
    fn controlled_stop_flushes_partial_tail_through_existing_eos_path() {
        let waker = futures_util::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let expected: Vec<_> = (0..777)
            .map(|frame| if frame % 2 == 0 { -0.0_f32 } else { 0.25_f32 })
            .collect();
        let source = Source {
            samples: expected.clone().into(),
            rate: 16_000,
            stopped: false,
        };
        let mut resampler = ResamplerDynamicNew::new(source, 16_000, 1_920).unwrap();
        assert!(Pin::new(&mut resampler).poll_next(&mut cx).is_pending());
        assert!(resampler.request_stop_source());
        let Poll::Ready(Some(Ok(tail))) = Pin::new(&mut resampler).poll_next(&mut cx) else {
            panic!("missing partial tail")
        };
        assert_eq!(
            tail.iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            Pin::new(&mut resampler).poll_next(&mut cx),
            Poll::Ready(None)
        ));
        let source = Source {
            samples: vec![0.25; 240].into(),
            rate: 48_000,
            stopped: false,
        };
        let mut resampler = ResamplerDynamicNew::new(source, 16_000, 1_920).unwrap();
        assert!(Pin::new(&mut resampler).poll_next(&mut cx).is_pending());
        assert!(resampler.request_stop_source());
        let mut output = Vec::new();
        loop {
            match Pin::new(&mut resampler).poll_next(&mut cx) {
                Poll::Ready(Some(Ok(chunk))) => output.extend(chunk),
                Poll::Ready(None) => break,
                result => panic!("resampling did not drain: {result:?}"),
            }
        }
        assert!(!output.is_empty() && output.len() <= 1_920);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 0.0));
    }
}
