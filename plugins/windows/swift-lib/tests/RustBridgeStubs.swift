// Native tests run without the Rust host that normally exports these callbacks.
@_cdecl("rust_on_floating_bar_stop")
func stubFloatingBarStop() {}
@_cdecl("rust_on_floating_bar_open_main")
func stubFloatingBarOpenMain() {}
@_cdecl("rust_on_floating_bar_settings_change")
func stubFloatingBarSettingsChange(_ pointer: UnsafePointer<CChar>) {}
@_cdecl("rust_on_devtools_panel_action")
func stubDevtoolsPanelAction(_ pointer: UnsafePointer<CChar>) {}
