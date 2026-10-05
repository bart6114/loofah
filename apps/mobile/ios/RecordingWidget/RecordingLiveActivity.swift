import ActivityKit
import SwiftUI
import UIKit
import WidgetKit

@main
struct LoofahRecordingWidgetBundle: WidgetBundle {
  var body: some Widget {
    RecordingLiveActivity()
  }
}

struct RecordingLiveActivity: Widget {
  var body: some WidgetConfiguration {
    ActivityConfiguration(for: LoofahRecordingAttributes.self) { context in
      RecordingLockScreen(attributes: context.attributes, state: context.state)
        .activityBackgroundTint(RecordingPalette.background)
        .activitySystemActionForegroundColor(RecordingPalette.accent)
    } dynamicIsland: { context in
      DynamicIsland {
        DynamicIslandExpandedRegion(.leading) {
          RecordingStatus(state: context.state)
            .font(.caption.weight(.medium))
            .foregroundStyle(.white)
        }
        DynamicIslandExpandedRegion(.trailing) {
          Text("loofah")
            .font(.system(.subheadline, design: .serif))
            .foregroundStyle(RecordingPalette.islandAccent)
        }
        DynamicIslandExpandedRegion(.bottom) {
          VStack(alignment: .leading, spacing: 12) {
            Text(context.attributes.title)
              .font(.headline)
              .lineLimit(2)
              .foregroundStyle(.white)
            HStack(alignment: .center, spacing: 16) {
              RecordingElapsed(state: context.state)
                .font(.system(.title2, design: .rounded).weight(.medium))
                .foregroundStyle(.white)
              Spacer(minLength: 0)
              RecordingStopButton(
                sessionID: context.attributes.sessionID, state: context.state, island: true)
            }
            if let message = context.state.message, context.state.phase != "recording" {
              Text(message)
                .font(.caption)
                .foregroundStyle(.white.opacity(0.8))
                .lineLimit(2)
            }
          }
          .padding(.top, 4)
          .padding(.bottom, 4)
        }
      } compactLeading: {
        RecordingMicrophone(state: context.state)
          .foregroundStyle(RecordingPalette.islandAccent)
      } compactTrailing: {
        RecordingElapsed(state: context.state)
          .font(.system(.caption2, design: .monospaced).weight(.medium))
          .frame(width: 62, alignment: .trailing)
          .foregroundStyle(.white)
      } minimal: {
        RecordingMicrophone(state: context.state)
          .foregroundStyle(RecordingPalette.islandAccent)
      }
      .keylineTint(RecordingPalette.islandAccent)
    }
  }
}

private struct RecordingLockScreen: View {
  var attributes: LoofahRecordingAttributes
  var state: LoofahRecordingAttributes.ContentState

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(alignment: .firstTextBaseline) {
        Text("loofah")
          .font(.system(.title3, design: .serif))
          .foregroundStyle(RecordingPalette.accent)
        Spacer(minLength: 12)
        RecordingStatus(state: state)
          .font(.caption.weight(.medium))
          .foregroundStyle(RecordingPalette.secondary)
      }

      Text(attributes.title)
        .font(.system(.headline, design: .serif))
        .foregroundStyle(RecordingPalette.primary)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)

      HStack(alignment: .center, spacing: 16) {
        VStack(alignment: .leading, spacing: 2) {
          RecordingElapsed(state: state)
            .font(.system(.largeTitle, design: .rounded).weight(.medium))
            .foregroundStyle(RecordingPalette.primary)
          Text("Recorded")
            .font(.caption)
            .foregroundStyle(RecordingPalette.secondary)
        }
        Spacer(minLength: 0)
        RecordingStopButton(sessionID: attributes.sessionID, state: state)
      }

      if let message = state.message, state.phase != "recording" {
        Text(message)
          .font(.caption)
          .foregroundStyle(RecordingPalette.secondary)
          .lineLimit(2)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(.horizontal, 20)
    .padding(.vertical, 16)
  }
}

private struct RecordingStatus: View {
  var state: LoofahRecordingAttributes.ContentState

  var body: some View {
    HStack(spacing: 6) {
      if state.phase == "recording" {
        Circle()
          .fill(Color(uiColor: .systemRed))
          .frame(width: 6, height: 6)
          .accessibilityHidden(true)
        Image(systemName: "waveform")
          .accessibilityHidden(true)
      } else {
        Image(systemName: state.phase == "stopping" ? "square.fill" : "pause.fill")
          .accessibilityHidden(true)
      }
      Text(
        state.phase == "recording"
          ? "Recording" : state.phase == "stopping" ? "Saving…" : "Interrupted"
      )
      .lineLimit(1)
    }
  }
}

private struct RecordingMicrophone: View {
  var state: LoofahRecordingAttributes.ContentState

  var body: some View {
    Image(
      systemName: state.phase == "recording"
        ? "mic.fill" : state.phase == "stopping" ? "square.fill" : "mic.slash.fill"
    )
    .accessibilityLabel(
      state.phase == "recording"
        ? "Recording" : state.phase == "stopping" ? "Saving recording" : "Recording interrupted")
  }
}

private struct RecordingElapsed: View {
  var state: LoofahRecordingAttributes.ContentState

  private var elapsed: Text {
    if state.phase == "recording", let anchor = state.timerAnchor {
      Text(timerInterval: anchor...Date.distantFuture, countsDown: false)
    } else {
      Text(Self.format(seconds: state.capturedSeconds))
    }
  }

  var body: some View {
    elapsed
      .monospacedDigit()
      .lineLimit(1)
      .minimumScaleFactor(0.7)
      .accessibilityLabel(Text("Recorded time"))
      .accessibilityValue(elapsed)
  }

  private static func format(seconds: Double) -> String {
    let total = Int(max(0, seconds.isFinite ? seconds : 0))
    if total >= 3600 {
      return String(format: "%d:%02d:%02d", total / 3600, total / 60 % 60, total % 60)
    }
    return String(format: "%d:%02d", total / 60, total % 60)
  }
}

private struct RecordingStopButton: View {
  var sessionID: String
  var state: LoofahRecordingAttributes.ContentState
  var island = false

  var body: some View {
    Button(intent: StopRecordingIntent(sessionID: sessionID)) {
      Label(state.phase == "stopping" ? "Saving…" : "Stop", systemImage: "stop.fill")
        .font(.subheadline.weight(.semibold))
        .padding(.horizontal, 18)
        .frame(minHeight: 44)
        .foregroundStyle(island ? Color.black : Color.white)
        .background(
          island ? RecordingPalette.islandAccent : RecordingPalette.buttonBackground, in: Capsule())
    }
    .buttonStyle(.plain)
    .disabled(state.phase == "stopping")
    .opacity(state.phase == "stopping" ? 0.65 : 1)
    .accessibilityLabel(state.phase == "stopping" ? "Saving recording" : "Stop and save recording")
  }
}

private enum RecordingPalette {
  static let background = Color(
    uiColor: UIColor { traits in
      traits.userInterfaceStyle == .dark
        ? UIColor(red: 0.10, green: 0.13, blue: 0.11, alpha: 1)
        : UIColor(red: 0.973, green: 0.969, blue: 0.957, alpha: 1)
    })
  static let accent = Color(
    uiColor: UIColor { traits in
      traits.userInterfaceStyle == .dark
        ? UIColor(red: 0.77, green: 0.89, blue: 0.78, alpha: 1)
        : UIColor(red: 0.212, green: 0.349, blue: 0.263, alpha: 1)
    })
  static let buttonBackground = Color(red: 0.212, green: 0.349, blue: 0.263)
  static let primary = Color(uiColor: .label)
  static let secondary = Color(uiColor: .secondaryLabel)
  static let islandAccent = Color(red: 0.77, green: 0.89, blue: 0.78)
}

#Preview(
  "Recording", as: .content,
  using: LoofahRecordingAttributes(sessionID: "preview", title: "Design catch-up")
) {
  RecordingLiveActivity()
} contentStates: {
  LoofahRecordingAttributes.ContentState(
    capturedSeconds: 125, timerAnchor: Date().addingTimeInterval(-125), phase: "recording")
}

#Preview(
  "Interrupted", as: .content,
  using: LoofahRecordingAttributes(
    sessionID: "preview", title: "A longer conversation about the next chapter of our product")
) {
  RecordingLiveActivity()
} contentStates: {
  LoofahRecordingAttributes.ContentState(
    capturedSeconds: 3665, timerAnchor: nil, phase: "interrupted",
    message: "Microphone interrupted. Open Loofah to check your recording.")
}
