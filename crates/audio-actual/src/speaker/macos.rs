mod resources;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;

use futures_util::Stream;
use futures_util::task::AtomicWaker;
use hypr_audio::Error;
use hypr_audio_interface::AsyncSource;
use hypr_audio_utils::{pcm_f64_to_f32, pcm_i16_to_f32, pcm_i32_to_f32};
use pin_project::pin_project;

use crate::async_ring::RingbufAsyncReader;
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::Split};

use ca::aggregate_device_keys as agg_keys;
use cidre::{arc, av, cat, cf, core_audio as ca, ns, os};

pub struct SpeakerInput {
    resources: resources::Resources,
    agg_desc: arc::Retained<cf::DictionaryOf<cf::String, cf::Type>>,
}

#[pin_project(PinnedDrop)]
pub struct SpeakerStream {
    reader: RingbufAsyncReader<HeapCons<f32>>,
    resources: resources::Resources,
    alive: Arc<AtomicBool>,
    integrity: Option<Arc<hypr_audio::CaptureIntegrity>>,

    current_sample_rate: u32,
    sample_rate_probe_counter: u32,
    buffer_rate: u32,
}

impl SpeakerStream {
    pub(crate) fn cleanup_failure(&self) -> Arc<AtomicBool> {
        self.resources.cleanup_failed.clone()
    }

    pub fn sample_rate(&self) -> u32 {
        self.buffer_rate
    }
}

struct Ctx {
    _owner: Option<Arc<dyn Send + Sync>>,
    common_format: av::audio::CommonFormat,
    producer: HeapProd<f32>,
    waker: Arc<AtomicWaker>,
    wake_pending: Arc<AtomicBool>,
    dropped_samples: Arc<AtomicUsize>,
    integrity: Option<Arc<hypr_audio::CaptureIntegrity>>,
    conversion_buffer: Vec<f32>,
}

use super::{BUFFER_SIZE, CHUNK_SIZE};

impl SpeakerInput {
    pub(crate) fn new_owned(owner: Option<Arc<dyn Send + Sync>>) -> Result<Self, Error> {
        let mut resources = resources::Resources::new(resources::CoreAudio, owner);
        let result = (|| {
            let tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new());
            let tap = tap_desc
                .create_process_tap()
                .map_err(|error| Error::SpeakerTapFailed(format!("create_process_tap: {error}")))?;
            resources::take_tap(&mut resources, tap);
            let tap_uid: arc::Retained<cf::String> = ca::Obj(resources.tap.unwrap())
                .cf_prop(&ca::PropSelector::TAP_UID.global_addr())
                .map_err(|error| Error::SpeakerTapFailed(format!("uid: {error}")))?;
            let sub_tap = cf::DictionaryOf::with_keys_values(
                &[ca::sub_device_keys::uid()],
                &[tap_uid.as_type_ref()],
            );
            Ok(cf::DictionaryOf::with_keys_values(
                &[
                    agg_keys::is_private(),
                    agg_keys::tap_auto_start(),
                    agg_keys::name(),
                    agg_keys::uid(),
                    agg_keys::tap_list(),
                ],
                &[
                    cf::Boolean::value_true().as_type_ref(),
                    cf::Boolean::value_false(),
                    cf::String::from_str(crate::TAP_DEVICE_NAME).as_ref(),
                    &cf::Uuid::new().to_cf_string(),
                    &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
                ],
            ))
        })();
        match result {
            Ok(agg_desc) => Ok(Self {
                resources,
                agg_desc,
            }),
            Err(error) => Err(resources::startup_error(&mut resources, error)),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        resources::tap_format(&self.resources).unwrap().sample_rate as u32
    }

    fn start_device(&mut self) -> Result<(), Error> {
        extern "C" fn proc(
            _device: ca::Device,
            _now: &cat::AudioTimeStamp,
            input_data: &cat::AudioBufList<1>,
            _input_time: &cat::AudioTimeStamp,
            _output_data: &mut cat::AudioBufList<1>,
            _output_time: &cat::AudioTimeStamp,
            ctx: Option<&mut Ctx>,
        ) -> os::Status {
            let ctx = ctx.unwrap();

            let first_buffer = &input_data.buffers[0];

            if first_buffer.data_bytes_size == 0 || first_buffer.data.is_null() {
                return os::Status::NO_ERR;
            }

            match ctx.common_format {
                av::audio::CommonFormat::PcmF32 => {
                    if let Some(samples) = read_samples::<f32>(first_buffer) {
                        process_audio_data_rt_safe(ctx, samples);
                    }
                }
                av::audio::CommonFormat::PcmF64 => {
                    process_samples_rt_safe::<f64>(ctx, first_buffer, pcm_f64_to_f32);
                }
                av::audio::CommonFormat::PcmI32 => {
                    process_samples_rt_safe::<i32>(ctx, first_buffer, pcm_i32_to_f32);
                }
                av::audio::CommonFormat::PcmI16 => {
                    process_samples_rt_safe::<i16>(ctx, first_buffer, pcm_i16_to_f32);
                }
                _ => {}
            }

            os::Status::NO_ERR
        }

        resources::start(&mut self.resources, &self.agg_desc, proc)
    }

    pub fn stream(self) -> Result<SpeakerStream, Error> {
        self.stream_with_integrity(None)
    }

    pub(crate) fn stream_with_integrity(
        self,
        integrity: Option<Arc<hypr_audio::CaptureIntegrity>>,
    ) -> Result<SpeakerStream, Error> {
        self.stream_owned(integrity, None)
    }
    pub(crate) fn stream_owned(
        mut self,
        integrity: Option<Arc<hypr_audio::CaptureIntegrity>>,
        owner: Option<Arc<dyn Send + Sync>>,
    ) -> Result<SpeakerStream, Error> {
        if self.resources.owner.is_none() {
            self.resources.owner = owner;
        }
        let result = self.prepare_stream(integrity.clone());
        let (reader, alive, sample_rate) = match result {
            Ok(value) => value,
            Err(error) => return Err(resources::startup_error(&mut self.resources, error)),
        };
        Ok(SpeakerStream {
            reader,
            resources: self.resources,
            alive,
            integrity,
            current_sample_rate: sample_rate,
            sample_rate_probe_counter: 0,
            buffer_rate: sample_rate,
        })
    }

    fn prepare_stream(
        &mut self,
        integrity: Option<Arc<hypr_audio::CaptureIntegrity>>,
    ) -> Result<(RingbufAsyncReader<HeapCons<f32>>, Arc<AtomicBool>, u32), Error> {
        let asbd = resources::tap_format(&self.resources)?;
        let format = av::AudioFormat::with_asbd(&asbd)
            .ok_or_else(|| Error::SpeakerTapFailed(format!("unsupported_asbd: {asbd:?}")))?;
        let common_format = format.common_format();

        let rb = HeapRb::<f32>::new(BUFFER_SIZE);
        let (producer, consumer) = rb.split();

        let waker = Arc::new(AtomicWaker::new());
        let wake_pending = Arc::new(AtomicBool::new(false));
        let current_sample_rate = asbd.sample_rate as u32;
        let dropped_samples = Arc::new(AtomicUsize::new(0));

        tracing::info!(init = asbd.sample_rate, "sample_rate");

        self.resources.context = Some(Box::new(Ctx {
            _owner: self.resources.owner.clone(),
            common_format,
            producer,
            waker: waker.clone(),
            wake_pending: wake_pending.clone(),
            dropped_samples: dropped_samples.clone(),
            integrity,
            conversion_buffer: vec![0.0f32; crate::rt_ring::DEFAULT_SCRATCH_LEN],
        }));

        self.start_device()?;
        let alive = Arc::new(AtomicBool::new(true));

        Ok((
            RingbufAsyncReader::new(consumer, waker, wake_pending, vec![0.0f32; CHUNK_SIZE])
                .with_dropped_samples(dropped_samples, "samples_dropped")
                .with_alive(alive.clone()),
            alive,
            current_sample_rate,
        ))
    }
}

fn read_samples<T: Copy>(buffer: &cat::AudioBuf) -> Option<&[T]> {
    let byte_count = buffer.data_bytes_size as usize;

    if byte_count == 0 || buffer.data.is_null() {
        return None;
    }

    let data = buffer.data as *const T;
    if !(data as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return None;
    }

    let sample_count = byte_count / std::mem::size_of::<T>();
    if sample_count == 0 {
        return None;
    }

    Some(unsafe { std::slice::from_raw_parts(data, sample_count) })
}

fn process_samples_rt_safe<T>(ctx: &mut Ctx, buffer: &cat::AudioBuf, convert: impl FnMut(T) -> f32)
where
    T: Copy + 'static,
{
    let Some(samples) = read_samples::<T>(buffer) else {
        return;
    };

    let stats = crate::rt_ring::convert_and_push_to_ringbuf(
        samples,
        &mut ctx.conversion_buffer,
        &mut ctx.producer,
        convert,
    );

    stats.record_integrity(ctx.integrity.as_deref(), false, true);
    if stats.dropped > 0 {
        ctx.dropped_samples
            .fetch_add(stats.dropped, Ordering::Relaxed);
    }

    if stats.pushed > 0 && ctx.wake_pending.load(Ordering::Acquire) {
        ctx.wake_pending.store(false, Ordering::Release);
        ctx.waker.wake();
    }
}

fn process_audio_data_rt_safe(ctx: &mut Ctx, data: &[f32]) {
    let stats = crate::rt_ring::push_f32_to_ringbuf(data, &mut ctx.producer);

    stats.record_integrity(ctx.integrity.as_deref(), false, true);
    if stats.dropped > 0 {
        ctx.dropped_samples
            .fetch_add(stats.dropped, Ordering::Relaxed);
    }

    if stats.pushed > 0 && ctx.wake_pending.load(Ordering::Acquire) {
        ctx.wake_pending.store(false, Ordering::Release);
        ctx.waker.wake();
    }
}

impl Stream for SpeakerStream {
    type Item = f32;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();

        if !this.reader.has_buffered_samples() {
            const SAMPLE_RATE_PROBE_INTERVAL: u32 = 128;
            this.sample_rate_probe_counter = this.sample_rate_probe_counter.wrapping_add(1);
            if this
                .sample_rate_probe_counter
                .is_multiple_of(SAMPLE_RATE_PROBE_INTERVAL)
            {
                if let Ok(asbd) = resources::tap_format(&this.resources) {
                    let after = asbd.sample_rate as u32;
                    if this.current_sample_rate != after {
                        this.current_sample_rate = after;
                    }
                }
            }
        }

        let res = this.reader.poll_next_sample(cx);
        if res.did_pop_chunk {
            this.buffer_rate = this.current_sample_rate;
        }

        res.poll
    }
}

impl AsyncSource for SpeakerStream {
    fn as_stream(&mut self) -> impl Stream<Item = f32> + '_ {
        self
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate()
    }
    fn request_stop(&mut self) -> bool {
        if let Err(error) = self.resources.close() {
            if let Some(integrity) = &self.integrity {
                integrity.mark_discontinuity(false, true);
            }
            tracing::error!(?error, "system_audio_stop_failed");
            return false;
        }
        self.alive.store(false, Ordering::Release);
        if let Some(context) = &self.resources.context {
            context.waker.wake();
        }
        true
    }
}

#[pin_project::pinned_drop]
impl PinnedDrop for SpeakerStream {
    fn drop(self: std::pin::Pin<&mut Self>) {
        let this = self.project();
        if this.resources.close().is_err() {
            if let Some(integrity) = this.integrity {
                integrity.mark_discontinuity(false, true);
            }
        }
    }
}
