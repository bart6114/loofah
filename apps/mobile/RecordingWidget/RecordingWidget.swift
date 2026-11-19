import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

@main
struct LoofahRecordingWidget: Widget {
  // The Dynamic Island uses a black background in both system appearances.
  private let islandTheme = CaptureTheme(.dark)

  var body: some WidgetConfiguration {
    ActivityConfiguration(for: CaptureRecordingAttributes.self) { context in
      RecordingCard(context: context)
    } dynamicIsland: { context in
      DynamicIsland {
        DynamicIslandExpandedRegion(.leading) {
          RecordingStatus(saving: context.state.savingAt != nil, theme: islandTheme)
            .lineLimit(1)
            .minimumScaleFactor(0.75)
        }
        DynamicIslandExpandedRegion(.trailing) {
          RecordingTimer(context: context)
            .font(.subheadline.weight(.semibold))
            .frame(width: 72, alignment: .trailing)
            .foregroundStyle(islandTheme.ink)
        }
        DynamicIslandExpandedRegion(.bottom) {
          if context.state.savingAt == nil {
            StopButton(id: context.attributes.captureID, theme: islandTheme, expands: true)
              .padding(.top, 8)
          }
        }
      } compactLeading: {
        RecordingMark(saving: context.state.savingAt != nil, theme: islandTheme, size: 16)
      } compactTrailing: {
        RecordingTimer(context: context)
          .font(.caption2.weight(.medium))
          .frame(width: 50, alignment: .trailing)
          .foregroundStyle(islandTheme.ink)
      } minimal: {
        RecordingMark(saving: context.state.savingAt != nil, theme: islandTheme, size: 16)
      }
      .contentMargins(.horizontal, 32, for: .expanded)
      .keylineTint(islandTheme.recording)
    }
  }
}

private struct RecordingMark: View {
  let saving: Bool
  let theme: CaptureTheme
  let size: CGFloat

  var body: some View {
    Image("LoofahRecordingMark")
      .renderingMode(.template)
      .resizable()
      .scaledToFit()
      .frame(width: size, height: size)
      .foregroundStyle(saving ? theme.muted : theme.recording)
      .accessibilityLabel(saving ? "Saving recording" : "Recording")
  }
}

private struct RecordingCard: View {
  let context: ActivityViewContext<CaptureRecordingAttributes>
  @Environment(\.colorScheme) private var scheme
  @Environment(\.dynamicTypeSize) private var typeSize
  @Environment(\.isLuminanceReduced) private var isLuminanceReduced

  var body: some View {
    let theme = CaptureTheme(isLuminanceReduced ? .dark : scheme)
    let layout =
      typeSize.isAccessibilitySize
      ? AnyLayout(VStackLayout(alignment: .leading, spacing: 12))
      : AnyLayout(HStackLayout(spacing: 16))

    layout {
      VStack(alignment: .leading, spacing: 4) {
        RecordingStatus(saving: context.state.savingAt != nil, theme: theme)
        RecordingTimer(context: context, alignment: .leading)
          .font(.title.weight(.semibold))
          .foregroundStyle(theme.ink)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      if context.state.savingAt == nil {
        StopButton(
          id: context.attributes.captureID, theme: theme, expands: typeSize.isAccessibilitySize)
      }
    }
    .padding(.horizontal, 18)
    .padding(.vertical, 12)
    // SpringBoard can apply material to the activity tint. Render the paper with its text.
    .background(theme.paper)
    .activityBackgroundTint(theme.paper)
    .activitySystemActionForegroundColor(theme.ink)
  }
}

private struct RecordingStatus: View {
  let saving: Bool
  let theme: CaptureTheme

  var body: some View {
    HStack(spacing: 6) {
      RecordingMark(saving: saving, theme: theme, size: 18)
        .accessibilityHidden(true)
      Text(saving ? "Saving…" : "Recording")
        .font(.subheadline.weight(.semibold))
    }
    .foregroundStyle(theme.ink)
  }
}

private struct RecordingTimer: View {
  let context: ActivityViewContext<CaptureRecordingAttributes>
  var alignment: TextAlignment = .trailing

  var body: some View {
    let start = context.attributes.startedAt
    Text(
      timerInterval: start...start.addingTimeInterval(8 * 60 * 60),
      pauseTime: context.state.savingAt, countsDown: false
    )
    .monospacedDigit()
    .multilineTextAlignment(alignment)
    .lineLimit(1)
    .minimumScaleFactor(0.7)
  }
}

private struct StopButton: View {
  let id: UUID
  let theme: CaptureTheme
  let expands: Bool

  var body: some View {
    Button(intent: StopCaptureRecording(captureID: id)) {
      Label("Stop", systemImage: "stop.fill")
        .font(.subheadline.weight(.semibold))
        .padding(.horizontal, 16)
        .frame(maxWidth: expands ? .infinity : nil, minHeight: 44)
        .contentShape(Rectangle())
    }
    .buttonStyle(CaptureButtonStyle(theme: theme, kind: .stop))
    .accessibilityLabel("Stop and save recording")
  }
}
