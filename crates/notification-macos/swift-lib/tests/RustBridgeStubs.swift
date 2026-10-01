// Swift package tests run without the Rust host that normally exports these callbacks.
var timedOutNotificationKeys: [String] = []
@_cdecl("rust_on_collapsed_confirm")
func stubCollapsedConfirm(_ key: UnsafePointer<CChar>, _ tag: Int32) {}

@_cdecl("rust_on_expanded_accept")
func stubExpandedAccept(_ key: UnsafePointer<CChar>, _ tag: Int32) {}

@_cdecl("rust_on_dismiss")
func stubDismiss(_ key: UnsafePointer<CChar>, _ tag: Int32) {}

@_cdecl("rust_on_collapsed_timeout")
func stubCollapsedTimeout(_ key: UnsafePointer<CChar>, _ tag: Int32) {
  timedOutNotificationKeys.append(String(cString: key))
}

@_cdecl("rust_on_expanded_start_time_reached")
func stubExpandedStartTimeReached(_ key: UnsafePointer<CChar>, _ tag: Int32) {}

@_cdecl("rust_on_option_selected")
func stubOptionSelected(_ key: UnsafePointer<CChar>, _ tag: Int32) {}

@_cdecl("rust_on_footer_action")
func stubFooterAction(_ key: UnsafePointer<CChar>, _ tag: Int32) {}
