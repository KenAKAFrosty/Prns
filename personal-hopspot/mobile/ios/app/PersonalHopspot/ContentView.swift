import SwiftUI

struct ContentView: View {
    @EnvironmentObject private var engine: EngineController
    @StateObject private var bridge = HopspotBridge()

    var body: some View {
        TimelineView(.periodic(from: .now, by: bridge.renderInterval)) { _ in
            Color.black
                .overlay {
                    if let frame = bridge.render() {
                        Image(decorative: frame, scale: 1.0)
                            .interpolation(.none)
                            .resizable()
                            .aspectRatio(contentMode: .fit)
                    }
                }
        }
        .ignoresSafeArea()
        .background(Color.black)
        .overlay(alignment: .top) {
            lifecycleOverlay
        }
        .gesture(
            LongPressGesture(minimumDuration: 0.50)
                .onEnded { _ in bridge.postLongPress() }
                .exclusively(
                    before: TapGesture()
                        .onEnded { bridge.postShortPress() }
                )
        )
    }

    @ViewBuilder
    private var lifecycleOverlay: some View {
        if engine.isStarting {
            Label("Starting Hopspot", systemImage: "hourglass")
                .modifier(LifecycleBadge())
        } else if engine.isFailed {
            Label(engine.failureDescription, systemImage: "exclamationmark.triangle.fill")
                .modifier(LifecycleBadge())
        }
    }
}

private struct LifecycleBadge: ViewModifier {
    func body(content: Content) -> some View {
        content
            .font(.footnote.weight(.semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.black.opacity(0.82), in: Capsule())
            .padding(.top, 12)
            .accessibilityAddTraits(.isStaticText)
    }
}

#Preview {
    ContentView()
        .environmentObject(EngineController.shared)
}
