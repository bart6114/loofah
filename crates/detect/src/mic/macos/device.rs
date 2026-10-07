use cidre::{arc, core_audio as ca, dispatch};
use std::sync::Weak;

use super::{AudioBackend, Control, DEVICE_IS_RUNNING_SOMEWHERE};

pub(super) struct CoreAudio {
    queue: arc::R<dispatch::Queue>,
}

impl CoreAudio {
    pub fn new() -> Self {
        Self {
            queue: dispatch::Queue::new(),
        }
    }
}

pub(super) struct Registration {
    object: ca::Obj,
    address: ca::PropAddr,
    queue: arc::R<dispatch::Queue>,
    block: arc::R<ca::PropListenerBlock>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Err(error) = self.object.remove_prop_listener_block(
            &self.address,
            Some(&self.queue),
            &mut self.block,
        ) {
            // CoreAudio owns its block copy. A failed removal retains only an inert weak wakeup.
            tracing::error!(?error, object = ?self.object, "removing_mic_listener_failed");
        }
    }
}

impl AudioBackend for CoreAudio {
    type Registration = Registration;
    fn default_input(&mut self) -> Option<ca::Obj> {
        ca::System::default_input_device()
            .ok()
            .map(|device| device.0)
    }
    fn register(
        &mut self,
        object: ca::Obj,
        address: ca::PropAddr,
        control: Weak<Control>,
    ) -> Result<Registration, ()> {
        let mut block = ca::PropListenerBlock::new2(move |_: u32, _: *const ca::PropAddr| {
            if let Some(control) = control.upgrade() {
                control.notify();
            }
        });
        object
            .add_prop_listener_block(&address, Some(&self.queue), &mut block)
            .map_err(|error| {
                tracing::error!(?error, ?object, "adding_mic_listener_failed");
            })?;
        Ok(Registration {
            object,
            address,
            queue: self.queue.clone(),
            block,
        })
    }
    fn is_running(&self, object: ca::Obj) -> Option<bool> {
        object
            .prop::<u32>(&DEVICE_IS_RUNNING_SOMEWHERE)
            .ok()
            .map(|value| value != 0)
    }
    fn drain(&self) {
        self.queue.sync(|| ());
    }
}
