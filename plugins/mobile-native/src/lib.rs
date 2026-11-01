use tauri::{Manager, Runtime, plugin::TauriPlugin};

mod error;
mod mobile;
mod models;

pub use error::{Error, Result};
pub use mobile::MobileNative;
pub use models::*;

pub trait MobileNativeExt<R: Runtime> {
    fn mobile_native(&self) -> &MobileNative<R>;
}

impl<R: Runtime, T: Manager<R>> MobileNativeExt<R> for T {
    fn mobile_native(&self) -> &MobileNative<R> {
        self.state::<MobileNative<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("mobile-native")
        .setup(|app, api| {
            app.manage(mobile::init(app, api)?);
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_types() {
        let mut types = specta::TypeCollection::default();
        types
            .register::<NativeWord>()
            .register::<NativeModelStatus>()
            .register::<NativeImportResult>()
            .register::<NativeVaultSelection>();
        specta_typescript::Typescript::default()
            .bigint(specta_typescript::BigIntExportBehavior::Number)
            .export_to("./js/bindings.gen.ts", &types)
            .unwrap();
    }
}
