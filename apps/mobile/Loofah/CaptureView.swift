import SwiftUI

struct CaptureView: View {
  @EnvironmentObject private var store: CaptureStore
  @Environment(\.colorScheme) private var scheme
  @Environment(\.dismiss) private var dismiss
  @Environment(\.dynamicTypeSize) private var typeSize
  @FocusState private var editorFocused: Bool
  @State private var confirmAccount = false
  @ScaledMetric(relativeTo: .title) private var headingSize = 34.0
  let id: UUID

  private var theme: CaptureTheme { CaptureTheme(scheme) }
  private var finishing: Bool { store.isFinishing(id) }

  var body: some View {
    if let item = store.captures.first(where: { $0.id == id }) {
      let recording = store.recording.flatMap { $0.id == id ? $0 : nil }
      let showControls = recording != nil || (finishing && item.capture.recording)
      let editable = store.canEdit(item)
      VStack(alignment: .leading, spacing: 18) {
        if editable {
          titleField
          if item.isQueued {
            deliveryDetails(item)
          } else if recording?.pausedAt != nil {
            recordingLabel("Recording paused")
          } else if item.capture.recording && !showControls {
            recordingLabel("Recording stopped. Tap Done to keep it.")
          }
          editor(item)
        } else {
          finished(item)
        }
      }
      .padding(.horizontal, 20)
      .padding(.top, 14)
      .frame(maxWidth: 640, maxHeight: .infinity, alignment: .topLeading)
      .frame(maxWidth: .infinity)
      .foregroundStyle(theme.ink)
      .background(theme.paper.ignoresSafeArea())
      .navigationTitle("")
      .toolbarVisibility(.visible, for: .navigationBar)
      .navigationBarTitleDisplayMode(.inline)
      .toolbarBackground(theme.paper, for: .navigationBar)
      .toolbarBackgroundVisibility(.visible, for: .navigationBar)
      .navigationBarBackButtonHidden(showControls || finishing)
      .toolbar {
        if editable && (!showControls || recording?.pausedAt != nil) {
          ToolbarItem(placement: .confirmationAction) {
            if finishing {
              ProgressView().accessibilityLabel("Saving capture")
            } else {
              Button("Done") {
                if item.isQueued {
                  store.sync()
                  dismiss()
                } else {
                  finish()
                }
              }
            }
          }
        }
      }
      .safeAreaInset(edge: .bottom, spacing: 0) {
        if showControls {
          recordingControls(
            recording?.timerStartedAt ?? item.capture.createdAt,
            pausedAt: recording?.pausedAt)
        }
      }
      .task {
        if !item.capture.recording && !item.isQueued && item.capture.note.isEmpty
          && item.capture.customTitle == nil
        {
          editorFocused = true
        }
      }
      .confirmationDialog(
        "Send with the current Apple Account?", isPresented: $confirmAccount,
        titleVisibility: .visible
      ) {
        Button("Send capture") { store.useCurrentAccount(id) }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text("This capture will go to the Mac set up with this account.")
      }
    }
  }

  private var titleField: some View {
    TextField(
      "Untitled",
      text: Binding(
        get: { store.captures.first(where: { $0.id == id })?.capture.title ?? "" },
        set: { store.edit(id, \.title, $0.isEmpty ? nil : $0) }
      )
    )
    .font(.system(size: headingSize, weight: .medium))
    .tracking(-0.8)
    .disabled(finishing)
    .accessibilityLabel("Title")
    .onSubmit { editorFocused = true }
  }

  private func editor(_ item: LocalCapture) -> some View {
    TextEditor(
      text: Binding(
        get: { store.captures.first(where: { $0.id == id })?.capture.note ?? "" },
        set: { store.edit(id, \.note, $0) }
      )
    )
    .font(.body)
    .lineSpacing(4)
    .scrollContentBackground(.hidden)
    .focused($editorFocused)
    .disabled(finishing)
    .accessibilityLabel("Notes")
    .padding(.horizontal, -5)
    .overlay(alignment: .topLeading) {
      if item.capture.note.isEmpty {
        Text(item.capture.recording ? "Add notes…" : "Write a note…")
          .font(.body)
          .foregroundStyle(theme.muted)
          .padding(.top, 8)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
  }

  private func finished(_ item: LocalCapture) -> some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 16) {
        Text(item.title)
          .font(.system(size: headingSize, weight: .medium))
          .tracking(-0.8)
          .accessibilityAddTraits(.isHeader)
        deliveryDetails(item)

        if !item.capture.note.isEmpty {
          Text(item.capture.note)
            .font(.body)
            .lineSpacing(4)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
      }
      .padding(.bottom, 20)
    }
  }

  private func deliveryDetails(_ item: LocalCapture) -> some View {
    let status = store.deliveryStatus(for: item)
    return VStack(alignment: .leading, spacing: 8) {
      VStack(alignment: .leading, spacing: 5) {
        Text(status.title).font(.subheadline.weight(.semibold))
        if let detail = status.detail {
          Text(detail).font(.footnote).foregroundStyle(theme.muted)
        }
        if status == .confirmAccount {
          Button {
            confirmAccount = true
          } label: {
            Text("Use this account")
              .font(.subheadline.weight(.semibold))
              .padding(.horizontal, 12)
              .frame(minHeight: 44)
          }
          .buttonStyle(CaptureButtonStyle(theme: theme, kind: .secondary))
          .padding(.top, 4)
        }
      }
      .padding(12)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(theme.wash, in: RoundedRectangle(cornerRadius: 8))

      if item.capture.recording {
        recordingLabel("Audio recording")
      }
    }
  }

  private func recordingLabel(_ label: LocalizedStringKey) -> some View {
    Label(label, systemImage: "waveform")
      .font(.subheadline)
      .foregroundStyle(theme.muted)
  }

  private func recordingControls(_ startedAt: Date, pausedAt: Date?) -> some View {
    let layout =
      typeSize.isAccessibilitySize
      ? AnyLayout(VStackLayout(alignment: .leading, spacing: 8))
      : AnyLayout(HStackLayout(spacing: 12))

    return layout {
      if finishing {
        HStack(spacing: 8) {
          ProgressView()
          Text("Saving…").font(.subheadline.weight(.medium))
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
      } else {
        TimelineView(.periodic(from: startedAt, by: 1)) { context in
          let elapsed = Duration.seconds(
            max(0, (pausedAt ?? context.date).timeIntervalSince(startedAt))
          )
          .formatted(.time(pattern: .minuteSecond))
          HStack(spacing: 8) {
            Circle().fill(pausedAt == nil ? theme.recording : theme.muted).frame(
              width: 8, height: 8)
            Text(elapsed).font(.body.monospacedDigit().weight(.medium))
          }
          .accessibilityElement(children: .ignore)
          .accessibilityLabel("Recording elapsed time")
          .accessibilityValue(elapsed)
          .frame(maxWidth: .infinity, alignment: .leading)
        }

        Button {
          if pausedAt != nil {
            store.resume(id)
          } else {
            finish()
          }
        } label: {
          Label(
            pausedAt == nil ? "Stop" : "Resume",
            systemImage: pausedAt == nil ? "stop.fill" : "play.fill"
          )
          .font(.subheadline.weight(.semibold))
          .padding(.horizontal, 16)
          .frame(minHeight: 44)
          .contentShape(Rectangle())
        }
        .buttonStyle(CaptureButtonStyle(theme: theme, kind: .stop))
      }
    }
    .padding(.horizontal, 20)
    .padding(.vertical, 8)
    .background(theme.paper)
    .overlay(alignment: .top) { Rectangle().fill(theme.line).frame(height: 1) }
  }

  private func finish() {
    Task {
      if await store.finish(id) { dismiss() }
    }
  }
}

struct HandoffView: View {
  @Environment(\.colorScheme) private var scheme
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    let theme = CaptureTheme(scheme)
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 16) {
          Text(
            "Install the latest Loofah for Mac. Open its Storage settings and turn on Receive iPhone captures. Use the same Apple Account with iCloud Drive on both devices."
          )
          Text(
            "You can edit a capture until your Mac starts importing it. Completed captures stay here until your Mac saves them. Transcription runs on your Mac."
          )
          Link("Privacy policy", destination: URL(string: "https://loofah.io/privacy-policy/")!)
            .fontWeight(.medium)
            .frame(minHeight: 44)
        }
        .font(.body)
        .foregroundStyle(theme.ink)
        .frame(maxWidth: 540, alignment: .leading)
        .padding(20)
        .frame(maxWidth: .infinity)
      }
      .background(theme.paper.ignoresSafeArea())
      .navigationTitle("Mac handoff")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .confirmationAction) {
          Button("Done") { dismiss() }
        }
      }
      .tint(theme.accent)
    }
  }
}
