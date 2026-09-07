#if DEBUG
import Foundation

@_cdecl("prns_app_ios_restoration_probe_emit")
func prnsAppIosRestorationProbeEmit(
  _ sequence: UInt64,
  _ codePointer: UnsafePointer<UInt8>?,
  _ codeLength: UInt
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
