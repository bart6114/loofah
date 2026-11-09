use super::super::teardown::{Failure, Operations, Owned, Stage};
use super::{Ctx, Error, ca, cat, cf};
use std::mem::ManuallyDrop;

type Callback = ca::hardware::DeviceIoProcId;
pub(super) type Resources = Owned<Callback, Ctx, CoreAudio>;
pub(super) struct CoreAudio;

// Signatures match the installed SDK AudioHardware.h and AudioHardwareTapping.h.
// cidre's consuming Drop paths hide release-build destruction failures.
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioDeviceStart(device: u32, callback: Option<Callback>) -> i32;
    fn AudioDeviceStop(device: u32, callback: Option<Callback>) -> i32;
    fn AudioDeviceCreateIOProcID(
        device: u32,
        proc: Callback,
        context: *mut std::ffi::c_void,
        callback: *mut Option<Callback>,
    ) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, callback: Callback) -> i32;
    fn AudioHardwareDestroyAggregateDevice(device: u32) -> i32;
    fn AudioHardwareDestroyProcessTap(tap: u32) -> i32;
}
impl Operations<Callback> for CoreAudio {
    fn stop(&self, device: u32, callback: Callback) -> i32 {
        unsafe { AudioDeviceStop(device, Some(callback)) }
    }
    fn destroy_callback(&self, device: u32, callback: Callback) -> i32 {
        unsafe { AudioDeviceDestroyIOProcID(device, callback) }
    }
    fn destroy_aggregate(&self, device: u32) -> i32 {
        unsafe { AudioHardwareDestroyAggregateDevice(device) }
    }
    fn destroy_tap(&self, tap: u32) -> i32 {
        unsafe { AudioHardwareDestroyProcessTap(tap) }
    }
}

pub(super) fn take_tap(resources: &mut Resources, tap: ca::TapGuard) {
    resources.tap = Some((**tap).0);
    let _owned = ManuallyDrop::new(tap);
}
pub(super) fn tap_format(resources: &Resources) -> Result<cat::AudioStreamBasicDesc, Error> {
    let tap = resources
        .tap
        .ok_or_else(|| Error::SpeakerTapFailed("Original tap is closed".into()))?;
    ca::Obj(tap)
        .prop(&ca::PropSelector::TAP_FORMAT.global_addr())
        .map_err(|error| Error::SpeakerTapFailed(format!("asbd: {error}")))
}
pub(super) fn start(
    resources: &mut Resources,
    description: &cf::DictionaryOf<cf::String, cf::Type>,
    proc: ca::hardware::DeviceIoProc<1, 1, Ctx>,
) -> Result<(), Error> {
    let aggregate = ca::AggregateDevice::with_desc(description)
        .map_err(|error| Error::SpeakerAggregateDeviceFailed(format!("with_desc: {error}")))?;
    let device = aggregate.as_ref().0.0;
    resources.device = Some(device);
    let _owned = ManuallyDrop::new(aggregate);
    let context = resources
        .context
        .as_mut()
        .expect("original callback context");
    let status = unsafe {
        AudioDeviceCreateIOProcID(
            device,
            std::mem::transmute::<ca::hardware::DeviceIoProc<1, 1, Ctx>, Callback>(proc),
            (&mut **context as *mut Ctx).cast(),
            &mut resources.callback,
        )
    };
    if status != 0 || resources.callback.is_none() {
        // A failed registration with no identity cannot prove that its callback
        // was never retained. Keep the original context rather than guessing.
        if resources.callback.is_none() {
            resources.uncertain(Failure {
                stage: Stage::Register,
                status: Some(status),
            });
        }
        return Err(Error::SpeakerAggregateDeviceFailed(format!(
            "create_io_proc_id: {status}"
        )));
    }
    // A failed start can still require stopping partially admitted native work.
    resources.started = true;
    let status = unsafe { AudioDeviceStart(device, resources.callback) };
    if status != 0 {
        return Err(Error::SpeakerAggregateDeviceFailed(format!(
            "device_start: {status}"
        )));
    }
    Ok(())
}

pub(super) fn startup_error(resources: &mut Resources, original: Error) -> Error {
    match resources.startup_error(original) {
        Ok(original) => original,
        Err((original, failure)) => {
            Error::CaptureCleanupUnconfirmed(format!("{original}; cleanup: {failure:?}"))
        }
    }
}
