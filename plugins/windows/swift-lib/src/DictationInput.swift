import AppKit
import ApplicationServices
import Carbon

@_silgen_name("rust_on_dictation_key_down")
private func rustOnDictationKeyDown()

@_silgen_name("rust_on_dictation_key_up")
private func rustOnDictationKeyUp()

@_silgen_name("rust_on_dictation_cancel")
private func rustOnDictationCancel()

final class DictationInput {
  static let shared = DictationInput()

  private var tap: CFMachPort?
  private var source: CFRunLoopSource?
  private var keyCode: CGKeyCode = 49
  private var modifiers: CGEventFlags = [.maskControl, .maskAlternate]
  private var pressed = false
  private var enabled = false
  private var active = false
  private var escapePressed = false
  private var target: AXUIElement?

  private init() {}

  func configure(_ shortcut: String) -> Bool {
    if shortcut.isEmpty {
      let suspend = {
        self.enabled = false
        self.pressed = false
        return true
      }
      if Thread.isMainThread { return suspend() }
      return DispatchQueue.main.sync(execute: suspend)
    }
    let parts = shortcut.lowercased().split(separator: "+").map(String.init)
    var flags: CGEventFlags = []
    for part in parts.dropLast() {
      switch part {
      case "ctrl", "control": flags.insert(.maskControl)
      case "alt", "option": flags.insert(.maskAlternate)
      case "shift": flags.insert(.maskShift)
      case "cmd", "command": flags.insert(.maskCommand)
      default: return false
      }
    }
    guard
      [CGEventFlags.maskControl, .maskAlternate, .maskShift, .maskCommand]
        .filter({ flags.contains($0) }).count >= 2
    else { return false }
    let install = {
      guard let key = parts.last.flatMap({ Self.keyCode(for: $0, modifiers: flags) }) else {
        return false
      }
      if self.tap != nil {
        let wasPressed =
          self.enabled && self.keyCode == key && self.modifiers == flags && self.pressed
        self.keyCode = key
        self.modifiers = flags
        self.enabled = true
        self.pressed = wasPressed
        return true
      }
      let mask =
        (1 << CGEventType.keyDown.rawValue)
        | (1 << CGEventType.keyUp.rawValue)
        | (1 << CGEventType.flagsChanged.rawValue)
      guard
        let tap = CGEvent.tapCreate(
          tap: .cgSessionEventTap,
          place: .headInsertEventTap,
          options: .defaultTap,
          eventsOfInterest: CGEventMask(mask),
          callback: { _, type, event, _ in
            DictationInput.shared.handle(type, event)
          },
          userInfo: nil
        )
      else { return false }
      self.keyCode = key
      self.modifiers = flags
      self.enabled = true
      self.pressed = false
      self.tap = tap
      let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
      self.source = source
      CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
      CGEvent.tapEnable(tap: tap, enable: true)
      return true
    }
    if Thread.isMainThread { return install() }
    return DispatchQueue.main.sync(execute: install)
  }

  func setActive(_ active: Bool) {
    if Thread.isMainThread {
      self.active = active
    } else {
      DispatchQueue.main.sync { self.active = active }
    }
  }

  private func handle(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
      if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
      return Unmanaged.passUnretained(event)
    }
    let eventKey = CGKeyCode(event.getIntegerValueField(.keyboardEventKeycode))
    if eventKey == 53 {
      if type == .keyUp && escapePressed {
        escapePressed = false
        return nil
      }
      if type == .keyDown && (active || escapePressed) {
        if !escapePressed {
          escapePressed = true
          rustOnDictationCancel()
        }
        return nil
      }
    }
    guard enabled else { return Unmanaged.passUnretained(event) }
    if type == .flagsChanged {
      if pressed && !event.flags.contains(modifiers) {
        pressed = false
        rustOnDictationKeyUp()
      }
      return Unmanaged.passUnretained(event)
    }
    guard eventKey == keyCode else { return Unmanaged.passUnretained(event) }
    if type == .keyDown && pressed { return nil }
    let activeModifiers = event.flags.intersection([
      .maskControl, .maskAlternate, .maskShift, .maskCommand,
    ])
    if type == .keyDown && activeModifiers == modifiers {
      if event.getIntegerValueField(.keyboardEventAutorepeat) != 0 { return nil }
      if !pressed {
        pressed = true
        rustOnDictationKeyDown()
      }
      return nil
    }
    if type == .keyUp && pressed {
      pressed = false
      rustOnDictationKeyUp()
      return nil
    }
    return Unmanaged.passUnretained(event)
  }

  func captureTarget() -> Int {
    if Thread.isMainThread { return captureTargetOnMain() }
    return DispatchQueue.main.sync { self.captureTargetOnMain() }
  }

  private func captureTargetOnMain() -> Int {
    guard AXIsProcessTrusted() else {
      target = nil
      return 1
    }
    guard let current = focusedElement(), editable(current) else {
      target = nil
      return 2
    }
    target = current
    return 0
  }

  func insert(_ text: String) -> Bool {
    if Thread.isMainThread { return insertOnMain(text) }
    return DispatchQueue.main.sync { self.insertOnMain(text) }
  }

  private func insertOnMain(_ text: String) -> Bool {
    guard let target, let current = focusedElement(), CFEqual(target, current), editable(current)
    else {
      return false
    }
    let board = NSPasteboard.general
    let previous = board.pasteboardItems?.map { item -> NSPasteboardItem in
      let copy = NSPasteboardItem()
      for type in item.types {
        if let data = item.data(forType: type) { copy.setData(data, forType: type) }
      }
      return copy
    }
    board.clearContents()
    guard board.setString(text, forType: .string) else { return false }
    let writtenChange = board.changeCount
    guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 9, keyDown: true),
      let up = CGEvent(keyboardEventSource: nil, virtualKey: 9, keyDown: false)
    else { return false }
    down.flags = .maskCommand
    up.flags = .maskCommand
    down.post(tap: .cghidEventTap)
    up.post(tap: .cghidEventTap)
    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(350)) {
      guard board.changeCount == writtenChange else { return }
      board.clearContents()
      if let previous, !previous.isEmpty { board.writeObjects(previous) }
    }
    self.target = nil
    return true
  }

  private func focusedElement() -> AXUIElement? {
    var result: CFTypeRef?
    let system = AXUIElementCreateSystemWide()
    guard
      AXUIElementCopyAttributeValue(system, kAXFocusedUIElementAttribute as CFString, &result)
        == .success
    else {
      return nil
    }
    return (result as! AXUIElement)
  }

  private func editable(_ element: AXUIElement) -> Bool {
    var roleValue: CFTypeRef?
    guard
      AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleValue) == .success,
      let role = roleValue as? String,
      role != "AXSecureTextField",
      ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"].contains(role)
    else { return false }
    var subroleValue: CFTypeRef?
    if AXUIElementCopyAttributeValue(element, kAXSubroleAttribute as CFString, &subroleValue)
      == .success,
      subroleValue as? String == kAXSecureTextFieldSubrole as String
    {
      return false
    }
    return true
  }

  private static func keyCode(for key: String, modifiers: CGEventFlags) -> CGKeyCode? {
    if key == "space" { return 49 }
    guard key.count == 1, key.range(of: "^[a-z0-9]$", options: .regularExpression) != nil,
      let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
      let pointer = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
    else { return nil }
    let data = Unmanaged<CFData>.fromOpaque(pointer).takeUnretainedValue()
    let layout = unsafeBitCast(CFDataGetBytePtr(data), to: UnsafePointer<UCKeyboardLayout>.self)
    for code in UInt16(0)..<128 {
      var deadKeyState: UInt32 = 0
      var length = 0
      var chars = [UniChar](repeating: 0, count: 4)
      let result = UCKeyTranslate(
        layout, code, UInt16(kUCKeyActionDown),
        modifiers.contains(.maskShift) ? 2 : 0, UInt32(LMGetKbdType()),
        OptionBits(kUCKeyTranslateNoDeadKeysMask), &deadKeyState, chars.count, &length, &chars)
      if result == noErr && String(utf16CodeUnits: chars, count: length).lowercased() == key {
        return code
      }
    }
    return nil
  }
}
