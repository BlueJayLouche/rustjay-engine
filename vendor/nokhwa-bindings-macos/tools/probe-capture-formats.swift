// Diagnostic for macOS capture-device problems.
//
//   swift vendor/nokhwa-bindings-macos/tools/probe-capture-formats.swift
//
// Dumps every capture device's formats in enumeration order, then replays the
// format/frame-rate-range selection from nokhwa-bindings-macos set_all() and
// flags any request where the chosen activeFormat and the chosen range would
// come from DIFFERENT formats — the mismatch that makes
// setActiveVideoMinFrameDuration raise NSInvalidArgumentException.
//
// Exits non-zero if it finds one. Read-only: never configures a device, so it
// is safe to run against hardware that is currently crashing an app.
//
// Run it with the problem device attached when someone reports a capture crash;
// the dump shows exactly which modes that device advertises.
import AVFoundation
import CoreMedia

let types: [AVCaptureDevice.DeviceType] = [
    .builtInWideAngleCamera, .external, .continuityCamera, .deskViewCamera,
]
let session = AVCaptureDevice.DiscoverySession(
    deviceTypes: types, mediaType: .video, position: .unspecified)

struct Fmt { let idx: Int; let w: Int32; let h: Int32; let codec: String; let rates: [(Double, CMTime)] }

var problems = 0

for device in session.devices {
    print("\n=== \(device.localizedName)  [\(device.deviceType.rawValue)] ===")
    var fmts: [Fmt] = []
    for (i, f) in device.formats.enumerated() {
        let d = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
        let sub = CMFormatDescriptionGetMediaSubType(f.formatDescription)
        let codec = String(bytes: [UInt8((sub >> 24) & 0xff), UInt8((sub >> 16) & 0xff),
                                  UInt8((sub >> 8) & 0xff), UInt8(sub & 0xff)], encoding: .ascii) ?? "????"
        let rates = f.videoSupportedFrameRateRanges.map { ($0.maxFrameRate, $0.minFrameDuration) }
        fmts.append(Fmt(idx: i, w: d.width, h: d.height, codec: codec, rates: rates))
        let rateStr = rates.map { String(format: "%.2f", $0.0) }.joined(separator: ",")
        print(String(format: "  [%2d] %5dx%-5d %@  maxFps: %@", i, d.width, d.height, codec, rateStr))
    }

    // Replay nokhwa's selection for every (resolution, fps) the device advertises.
    var seen = Set<String>()
    for f in fmts {
        for (fps, _) in f.rates {
            let key = "\(f.w)x\(f.h)@\(Int(fps.rounded()))"
            if seen.contains(key) { continue }
            seen.insert(key)

            var selFmt: Int? = nil
            var selRangeFmt: Int? = nil
            outer: for g in fmts where g.w == f.w && g.h == f.h {
                for (maxFps, _) in g.rates where abs(fps - maxFps) < 0.999 {
                    selFmt = g.idx                               // FIXED: both together
                    selRangeFmt = g.idx
                    break outer
                }
            }
            if let a = selFmt, let b = selRangeFmt, a != b {
                problems += 1
                let af = fmts.first { $0.idx == a }!
                let bf = fmts.first { $0.idx == b }!
                print("  !! \(key): activeFormat=[\(a)] \(af.codec) (max \(af.rates.map{$0.0}.max() ?? 0)fps)"
                    + " but duration from [\(b)] \(bf.codec) (\(fps)fps)  -> WOULD RAISE")
            }
        }
    }
}
print("\nmismatches found: \(problems)")
exit(problems == 0 ? 0 : 1)
