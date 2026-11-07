import Foundation
import SwiftRs

@_cdecl("_floating_bar_show")
public func _floatingBarShow() -> Bool {
  FloatingBarManager.shared.show()
  return true
}

@_cdecl("_floating_bar_hide")
public func _floatingBarHide() -> Bool {
  FloatingBarManager.shared.hide()
  return true
}

@_cdecl("_floating_bar_update")
public func _floatingBarUpdate(json: SRString) -> Bool {
  let jsonString = json.toString()
  guard let data = jsonString.data(using: .utf8),
    let payload = try? JSONDecoder().decode(FloatingBarStatePayload.self, from: data)
  else {
    return false
  }

  FloatingBarManager.shared.update(state: payload)
  return true
}

@_cdecl("_live_caption_show")
public func _liveCaptionShow() -> Bool {
  LiveCaptionManager.shared.show()
  return true
}

@_cdecl("_live_caption_hide")
public func _liveCaptionHide() -> Bool {
  LiveCaptionManager.shared.hide()
  return true
}

@_cdecl("_live_caption_update")
public func _liveCaptionUpdate(json: SRString) -> Bool {
  let jsonString = json.toString()
  guard let data = jsonString.data(using: .utf8),
    let payload = try? JSONDecoder().decode(LiveCaptionStatePayload.self, from: data)
  else {
    return false
  }

  LiveCaptionManager.shared.update(state: payload)
  return true
}

@_cdecl("_devtools_panel_show")
public func _devtoolsPanelShow() -> Bool {
  DevtoolsPanelManager.shared.show()
  return true
}

@_cdecl("_devtools_panel_hide")
public func _devtoolsPanelHide() -> Bool {
  DevtoolsPanelManager.shared.hide()
  return true
}

@_cdecl("_floating_bar_nudge")
public func _floatingBarNudge(json: SRString) -> Bool {
  guard Thread.isMainThread,
    let data = json.toString().data(using: .utf8),
    let payload = try? JSONDecoder().decode(FloatingNudgePayload.self, from: data)
  else { return false }
  return FloatingBarManager.shared.showNudge(payload)
}

@_cdecl("_floating_bar_clear_nudge")
public func _floatingBarClearNudge(recording_id: SRString) -> Bool {
  guard Thread.isMainThread else { return false }
  let id = recording_id.toString()
  FloatingBarManager.shared.clearNudge(recordingId: id.isEmpty ? nil : id)
  return true
}
