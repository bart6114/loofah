use std::{sync::mpsc, time::Duration};

use hypr_audio_device::AudioDeviceBackend;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    eAll, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{PCWSTR, Result, implement};

use crate::{DeviceEvent, DeviceSwitch, DeviceUpdate};

#[implement(IMMNotificationClient)]
struct DeviceCallback(mpsc::Sender<DeviceEvent>);

impl DeviceCallback {
    fn changed(&self) -> Result<()> {
        let _ = self
            .0
            .send(DeviceEvent::Switch(DeviceSwitch::DeviceListChanged));
        Ok(())
    }
}

#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        self.changed()
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        self.changed()
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        self.changed()
    }

    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
        if role == eConsole {
            let event = if flow == eCapture {
                Some(DeviceSwitch::DefaultInputChanged)
            } else if flow == eRender {
                Some(DeviceSwitch::DefaultOutputChanged { headphone: None })
            } else {
                None
            };
            if let Some(event) = event {
                let _ = self.0.send(DeviceEvent::Switch(event));
            }
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        self.changed()
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    id: String,
    tx: mpsc::Sender<DeviceEvent>,
}

#[allow(non_snake_case)]
impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        // Windows owns this pointer, valid only during the callback.
        if let Some(data) = unsafe { data.as_ref() } {
            let _ = self
                .tx
                .send(DeviceEvent::Update(DeviceUpdate::VolumeChanged {
                    device_uid: self.id.clone(),
                    volume: data.fMasterVolume,
                }));
            let _ = self.tx.send(DeviceEvent::Update(DeviceUpdate::MuteChanged {
                device_uid: self.id.clone(),
                is_muted: data.bMuted.as_bool(),
            }));
        }
        Ok(())
    }
}

struct VolumeSubscription {
    endpoint: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Drop for VolumeSubscription {
    fn drop(&mut self) {
        let _ = unsafe { self.endpoint.UnregisterControlChangeNotify(&self.callback) };
    }
}

fn subscribe_volumes(
    enumerator: &IMMDeviceEnumerator,
    tx: &mpsc::Sender<DeviceEvent>,
) -> Result<Vec<VolumeSubscription>> {
    let devices = unsafe { enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)? };
    let mut subscriptions = Vec::new();
    for index in 0..unsafe { devices.GetCount()? } {
        let subscription = (|| -> Result<_> {
            let device = unsafe { devices.Item(index)? };
            let raw_id = unsafe { device.GetId()? };
            let id = unsafe { raw_id.to_string() };
            unsafe { CoTaskMemFree(Some(raw_id.0.cast())) };
            let endpoint = unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)? };
            let callback: IAudioEndpointVolumeCallback = VolumeCallback {
                id: id?,
                tx: tx.clone(),
            }
            .into();
            unsafe { endpoint.RegisterControlChangeNotify(&callback)? };
            Ok(VolumeSubscription { endpoint, callback })
        })();
        match subscription {
            Ok(subscription) => subscriptions.push(subscription),
            Err(error) => tracing::debug!(%error, "endpoint_volume_subscription_unavailable"),
        }
    }
    Ok(subscriptions)
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn run(stop_rx: mpsc::Receiver<()>, mut emit: impl FnMut(DeviceEvent) -> bool, volumes: bool) {
    let result = (|| -> Result<()> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        let _apartment = ComApartment;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let (tx, rx) = mpsc::channel();
        let callback: IMMNotificationClient = DeviceCallback(tx.clone()).into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&callback)? };
        let mut subscriptions = if volumes {
            subscribe_volumes(&enumerator, &tx).unwrap_or_default()
        } else {
            Vec::new()
        };
        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let Ok(mut event) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            if let DeviceEvent::Switch(DeviceSwitch::DefaultOutputChanged { headphone }) =
                &mut event
            {
                let backend = hypr_audio_device::backend();
                *headphone = backend
                    .get_default_output_device()
                    .ok()
                    .flatten()
                    .map(|device| backend.is_headphone(&device));
            }
            if volumes && matches!(event, DeviceEvent::Switch(_)) {
                subscriptions.clear();
                subscriptions = subscribe_volumes(&enumerator, &tx).unwrap_or_default();
            }
            if !emit(event) {
                break;
            }
        }
        subscriptions.clear();
        unsafe { enumerator.UnregisterEndpointNotificationCallback(&callback)? };
        Ok(())
    })();
    if let Err(error) = result {
        tracing::error!(%error, "windows_device_monitor_failed");
    }
}

pub(crate) fn monitor_device_change(tx: mpsc::Sender<DeviceSwitch>, stop_rx: mpsc::Receiver<()>) {
    run(
        stop_rx,
        |event| match event {
            DeviceEvent::Switch(event) => tx.send(event).is_ok(),
            _ => true,
        },
        false,
    );
}

pub(crate) fn monitor_volume_mute(tx: mpsc::Sender<DeviceUpdate>, stop_rx: mpsc::Receiver<()>) {
    run(
        stop_rx,
        |event| match event {
            DeviceEvent::Update(event) => tx.send(event).is_ok(),
            _ => true,
        },
        true,
    );
}

pub(crate) fn monitor(tx: mpsc::Sender<DeviceEvent>, stop_rx: mpsc::Receiver<()>) {
    run(stop_rx, |event| tx.send(event).is_ok(), true);
}
