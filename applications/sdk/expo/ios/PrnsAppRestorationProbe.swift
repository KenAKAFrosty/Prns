#if DEBUG
import Foundation

enum PrnsAppRestorationProbe {
  static func install() {
    #if os(iOS)
    prns_app_ios_install_restoration_probe(prnsAppIosRestorationProbeEmit)
    #endif
  }
}

@_cdecl("prns_app_ios_restoration_probe_emit")
func prnsAppIosRestorationProbeEmit(
  _ sequence: UInt64,
  _ codePointer: UnsafePointer<UInt8>?,
  _ codeLength: Int
) {
  guard codeLength > 0, codeLength <= 64, let codePointer else {
    return
  }
  let code = String(decoding: UnsafeBufferPointer(start: codePointer, count: Int(codeLength)), as: UTF8.self)
  guard let event = PrnsIosDiagnostics.RestorationEvent(rawValue: code) else {
    return
  }
  PrnsIosDiagnostics.restoration(sequence: sequence, event: event)
}
#endif
