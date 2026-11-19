import SwiftUI

// Mirrors Default in apps/desktop/src/shared/theme/catalog.ts.
struct CaptureTheme {
  let shell: Color
  let paper: Color
  let ink: Color
  let muted: Color
  let line: Color
  let accent: Color
  let accentInk: Color
  let wash: Color
  let selection: Color
  let recording: Color
  let recordingInk: Color

  init(_ scheme: ColorScheme) {
    let dark = scheme == .dark
    shell = Color(hex: dark ? 0x171717 : 0xF5F5F5)
    paper = Color(hex: dark ? 0x212121 : 0xFFFFFF)
    ink = Color(hex: dark ? 0xEDEDED : 0x1C1C1C)
    muted = Color(hex: dark ? 0xA3A3A3 : 0x666666)
    line = Color(hex: dark ? 0x383838 : 0xE5E5E5)
    accent = Color(hex: dark ? 0xEDEDED : 0x292929)
    accentInk = Color(hex: dark ? 0x171717 : 0xFFFFFF)
    wash = Color(hex: dark ? 0x292929 : 0xEDEDED)
    selection = Color(hex: dark ? 0x2E2E2E : 0xE3E3E3)
    recording = Color(hex: dark ? 0xF1867E : 0xC0342A)
    recordingInk = Color(hex: dark ? 0x2E1110 : 0xFFFFFF)
  }
}

extension Color {
  fileprivate init(hex: UInt32) {
    self.init(
      red: Double((hex >> 16) & 255) / 255,
      green: Double((hex >> 8) & 255) / 255,
      blue: Double(hex & 255) / 255)
  }
}

struct CaptureButtonStyle: ButtonStyle {
  enum Kind {
    case primary, secondary, stop, row
  }

  let theme: CaptureTheme
  let kind: Kind
  @Environment(\.isEnabled) private var enabled

  func makeBody(configuration: Configuration) -> some View {
    let pressed = configuration.isPressed
    let foreground =
      kind == .primary ? theme.accentInk : kind == .stop ? theme.recordingInk : theme.ink
    let background =
      switch kind {
      case .primary: theme.accent
      case .secondary: theme.wash
      case .stop: theme.recording
      case .row: pressed ? theme.selection : Color.clear
      }

    configuration.label
      .foregroundStyle(foreground)
      .background(background, in: RoundedRectangle(cornerRadius: kind == .row ? 6 : 8))
      .overlay {
        if pressed && kind != .row {
          RoundedRectangle(cornerRadius: 8).fill(foreground.opacity(0.14))
        }
      }
      .opacity(enabled ? 1 : 0.55)
  }
}
