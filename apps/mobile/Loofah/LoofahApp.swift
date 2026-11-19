import SwiftUI
import UIKit

@main
struct LoofahApp: App {
  @UIApplicationDelegateAdaptor(CaptureAppDelegate.self) private var delegate
  @StateObject private var store = CaptureStore.shared

  var body: some Scene {
    WindowGroup {
      InboxView()
        .environmentObject(store)
    }
  }
}

private struct InboxView: View {
  @EnvironmentObject private var store: CaptureStore
  @Environment(\.scenePhase) private var phase
  @Environment(\.colorScheme) private var scheme
  @Environment(\.dynamicTypeSize) private var typeSize
  @Environment(\.openURL) private var openURL
  @State private var path: [UUID] = []
  @State private var showHandoff = false

  private var theme: CaptureTheme { CaptureTheme(scheme) }

  var body: some View {
    NavigationStack(path: $path) {
      ScrollView {
        VStack(alignment: .leading, spacing: 16) {
          HStack(spacing: 9) {
            Image("LoofahMark")
              .resizable()
              .scaledToFit()
              .frame(width: 34, height: 34)
              .accessibilityHidden(true)
            Text("loofah")
              .font(.title.weight(.semibold))
              .tracking(-0.9)
              .accessibilityAddTraits(.isHeader)
          }

          let layout =
            typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(spacing: 8)) : AnyLayout(HStackLayout(spacing: 8))
          layout {
            action("Record", icon: "mic", audio: true)
            action("New note", icon: "square.and.pencil", audio: false)
          }
          .disabled(store.busy)

          VStack(alignment: .leading, spacing: 0) {
            HStack {
              Text("On this iPhone")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.muted)
                .accessibilityAddTraits(.isHeader)
              Spacer()
              Button {
                showHandoff = true
              } label: {
                Image(systemName: "info.circle")
                  .font(.body)
                  .foregroundStyle(theme.muted)
                  .frame(width: 44, height: 44, alignment: .trailing)
                  .contentShape(Rectangle())
              }
              .buttonStyle(.plain)
              .accessibilityLabel("Mac handoff")
            }
            captures
          }
        }
        .frame(maxWidth: 540)
        .padding(.horizontal, 20)
        .padding(.top, 12)
        .padding(.bottom, 16)
        .frame(maxWidth: .infinity)
      }
      .background(theme.shell.ignoresSafeArea())
      .toolbarVisibility(.hidden, for: .navigationBar)
      .navigationDestination(for: UUID.self) { id in
        CaptureView(id: id)
      }
    }
    .foregroundStyle(theme.ink)
    .tint(theme.accent)
    .onChange(of: phase) { _, phase in
      if phase == .active { store.sync() }
      if phase == .background { CaptureAppDelegate.scheduleHandoff() }
    }
    .onChange(of: path) { old, new in
      for id in old where !new.contains(id) { store.discardEmptyNote(id) }
    }
    .onChange(of: store.captures.map(\.id)) {
      if let current = path.last, !store.captures.contains(where: { $0.id == current }) {
        path.removeAll()
      }
    }
    .sheet(isPresented: $showHandoff) {
      HandoffView()
    }
    .alert(
      store.problem?.title ?? "Capture",
      isPresented: Binding(
        get: { store.problem != nil },
        set: { if !$0 { store.problem = nil } }
      ), presenting: store.problem
    ) { problem in
      if problem == .microphone {
        Button("Open Settings") {
          if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
        }
        Button("Cancel", role: .cancel) {}
      } else {
        Button("OK", role: .cancel) {}
      }
    } message: { problem in
      Text(problem.message)
    }
  }

  private var captures: some View {
    LazyVStack(spacing: 0) {
      if store.captures.isEmpty {
        Text("No captures")
          .font(.subheadline)
          .foregroundStyle(theme.muted)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(12)
      }
      ForEach(store.captures) { item in
        if item.id != store.captures.first?.id {
          Rectangle()
            .fill(theme.line)
            .frame(height: 1)
            .padding(.horizontal, 10)
        }
        captureRow(item)
      }
    }
    .padding(3)
    .background(theme.paper, in: RoundedRectangle(cornerRadius: 8))
    .overlay { RoundedRectangle(cornerRadius: 8).strokeBorder(theme.line) }
  }

  private func captureRow(_ item: LocalCapture) -> some View {
    let timestamp = item.capture.createdAt.formatted(
      .dateTime.month(.abbreviated).day().hour().minute())
    let status: String
    if store.isFinishing(item.id) {
      status = "Saving…"
    } else {
      status = item.receipt == nil ? "Draft" : store.deliveryStatus(for: item).title
    }
    let hasRecording = item.capture.recording
    let hasNote = item.capture.note.contains { !$0.isWhitespace }
    let contents = [hasRecording ? "Recording" : nil, hasNote ? "Note" : nil].compactMap { $0 }

    return NavigationLink(value: item.id) {
      HStack(spacing: 11) {
        VStack(alignment: .leading, spacing: 4) {
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(item.title)
              .font(.body.weight(.medium))
              .lineLimit(2)
            if hasRecording {
              Image(systemName: "waveform")
                .font(.caption2)
                .foregroundStyle(theme.muted)
                .accessibilityHidden(true)
            }
            if hasNote {
              Image(systemName: "doc.text")
                .font(.caption2)
                .foregroundStyle(theme.muted)
                .accessibilityHidden(true)
            }
          }
          ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) {
              Text(timestamp)
              Text("·")
              Text(status)
            }
            .fixedSize(horizontal: true, vertical: false)
            VStack(alignment: .leading, spacing: 2) {
              Text(timestamp)
              Text(status)
            }
          }
          .font(.caption)
          .foregroundStyle(theme.muted)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        Image(systemName: "chevron.forward")
          .font(.caption2.weight(.semibold))
          .foregroundStyle(theme.muted)
          .accessibilityHidden(true)
      }
      .padding(.horizontal, 10)
      .padding(.vertical, 9)
      .frame(minHeight: 44)
      .contentShape(Rectangle())
    }
    .buttonStyle(CaptureButtonStyle(theme: theme, kind: .row))
    .accessibilityLabel(item.title)
    .accessibilityValue((contents + [status, timestamp]).joined(separator: ", "))
  }

  private func action(_ label: LocalizedStringKey, icon: String, audio: Bool) -> some View {
    Button {
      Task {
        if let id = await store.create(recording: audio) { path.append(id) }
      }
    } label: {
      Label(label, systemImage: icon)
        .font(.subheadline.weight(.semibold))
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, minHeight: 46)
        .contentShape(Rectangle())
    }
    .buttonStyle(CaptureButtonStyle(theme: theme, kind: audio ? .primary : .secondary))
  }
}
