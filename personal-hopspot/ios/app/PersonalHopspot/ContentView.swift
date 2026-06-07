//WIP NEEDS REVIEW
import SwiftUI

struct ContentView: View {
    @State private var bridge = HopspotBridge()

    var body: some View {
        TimelineView(.animation) { _ in
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
        .gesture(
            LongPressGesture(minimumDuration: 0.65)
                .onEnded { _ in bridge.postInput(HopspotBridge.inputLongPress) }
                .exclusively(
                    before: TapGesture()
                        .onEnded { bridge.postInput(HopspotBridge.inputShortPress) }
                )
        )
    }
}

#Preview {
    ContentView()
}
