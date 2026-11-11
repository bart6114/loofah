import AppKit
import ApplicationServices

@_silgen_name("rust_on_dictation_key_down")
private func rustOnDictationKeyDown()

@_silgen_name("rust_on_dictation_key_up")
private func rustOnDictationKeyUp()

final class DictationInput {
  static let shared = DictationInput()

  private var tap: CFMachPort?
  private var source: CFRunLoopSource?
  private var keyCode: CGKeyCode = 49
  private var modifiers: CGEventFlags = [.maskControl, .maskAlternate]
  private var pressed = false
  private var target: AXUIElement?

  private init() {}

  func configure(_ shortcut: String) -> Bool {
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
    guard let key = parts.last.flatMap(Self.keyCode(for:)) else { return false }
    let install = {
      self.keyCode = key
      self.modifiers = flags
      if self.tap != nil { return true }
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

  private func handle(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
    if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
      if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
      return Unmanaged.passUnretained(event)
    }
    if type == .flagsChanged {
      if pressed && !event.flags.contains(modifiers) {
        pressed = false
        rustOnDictationKeyUp()
      }
      return Unmanaged.passUnretained(event)
    }
    let eventKey = CGKeyCode(event.getIntegerValueField(.keyboardEventKeycode))
    guard eventKey == keyCode else { return Unmanaged.passUnretained(event) }
    if type == .keyDown && pressed { return nil }
    let activeModifiers = event.flags.intersection([
      .maskControl, .maskAlternate, .maskShift, .maskCommand,
    ])
    if type == .keyDown && activeModifiers == modifiers {
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
      let options =
        [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
      AXIsProcessTrustedWithOptions(options)
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
    return true
  }

  private static func keyCode(for key: String) -> CGKeyCode? {
    let keys: [String: CGKeyCode] = [
      "space": 49, "a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3,
      "g": 5, "h": 4, "i": 34, "j": 38, "k": 40, "l": 37, "m": 46,
      "n": 45, "o": 31, "p": 35, "q": 12, "r": 15, "s": 1, "t": 17,
      "u": 32, "v": 9, "w": 13, "x": 7, "y": 16, "z": 6,
      "0": 29, "1": 18, "2": 19, "3": 20, "4": 21, "5": 23,
      "6": 22, "7": 26, "8": 28, "9": 25,
    ]
    return keys[key]
  }
}
