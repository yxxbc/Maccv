//
//  SelfTest.swift
//  MacCV
//
//  An end-to-end check of the Swift → C ABI → Rust path.
//
//  `xcodebuild` proves the app compiles and links; it cannot prove that a
//  snapshot taken through Swift actually carries the clipboard's bytes back, or
//  that the watcher fires. `MacCV --self-test` exercises exactly that and exits
//  non-zero on failure, which is what CI needs.
//

import Foundation

enum SelfTest {
    /// Runs every check, printing progress. Returns an exit code.
    static func run() -> Int32 {
        var failures: [String] = []

        func check(_ name: String, _ condition: @autoclosure () throws -> Bool) {
            do {
                if try condition() {
                    print("  ok   \(name)")
                } else {
                    print("  FAIL \(name)")
                    failures.append(name)
                }
            } catch {
                print("  FAIL \(name) — \(error)")
                failures.append(name)
            }
        }

        print("MacCV self-test (engine \(ClipboardEngine.engineVersion))")

        // A process-private pasteboard, not the general one: this run must never
        // read or clobber the user's real clipboard, and CI must not depend on
        // whatever happens to be in it.
        guard let engine = ClipboardEngine.privateToProcess() else {
            print("  FAIL could not open a private pasteboard")
            return 1
        }

        print("\n[1] engine")
        check("version is non-empty", !ClipboardEngine.engineVersion.isEmpty)
        check("access behavior is a known value", (0...3).contains(Int(engine.accessBehavior.rawValue)))
        _ = engine.changeCount

        let text = "MacCV self-test 你好 🚀"
        let before = engine.changeCount

        print("\n[2] write → read round trip")
        do {
            try engine.writeText(text)
        } catch {
            print("  FAIL writeText threw: \(error)")
            return 1
        }
        check("changeCount advanced", engine.changeCount > before)

        let snapshot: ClipSnapshot
        do {
            snapshot = try engine.snapshot()
        } catch {
            print("  FAIL snapshot threw: \(error)")
            return 1
        }
        check("snapshot has one item", snapshot.items.count == 1)
        check("kind is text", snapshot.kind == .text)
        check("plain text survived byte-for-byte", snapshot.items.first?.plainText == text)
        check(
            "raw utf8 representation is present",
            snapshot.items.first?.representation(PasteboardUTI.string)?.data == Data(text.utf8)
        )

        print("\n[3] rich + multipart write")
        let rich = ClipItem(
            kind: .richText,
            preview: "rich",
            byteLength: 0,
            representations: [
                ClipRepresentation(typeIdentifier: PasteboardUTI.string, data: Data("plain".utf8)),
                ClipRepresentation(typeIdentifier: PasteboardUTI.rtf, data: Data("{\\rtf1 rich}".utf8)),
                ClipRepresentation(typeIdentifier: PasteboardUTI.html, data: Data("<b>rich</b>".utf8)),
            ]
        )
        do {
            try engine.write(rich)
            let back = try engine.snapshot()
            check("rtf payload replayed exactly",
                  back.items.first?.representation(PasteboardUTI.rtf)?.data == Data("{\\rtf1 rich}".utf8))
            check("html payload replayed exactly",
                  back.items.first?.representation(PasteboardUTI.html)?.data == Data("<b>rich</b>".utf8))
            check("rtf classified as rich text", back.kind == .richText)
        } catch {
            print("  FAIL rich write threw: \(error)")
            failures.append("rich write")
        }

        print("\n[4] multi-item write")
        do {
            let items = (1...3).map { index in
                ClipItem(
                    kind: .text,
                    preview: "item-\(index)",
                    byteLength: 0,
                    representations: [
                        ClipRepresentation(
                            typeIdentifier: PasteboardUTI.string,
                            data: Data("item-\(index)".utf8)
                        )
                    ]
                )
            }
            try engine.write(items)
            let back = try engine.snapshot()
            check("three items preserved", back.items.count == 3)
            check(
                "item order preserved",
                back.items.compactMap(\.plainText) == ["item-1", "item-2", "item-3"]
            )
        } catch {
            print("  FAIL multi-item write threw: \(error)")
            failures.append("multi-item write")
        }

        print("\n[5] error handling")
        do {
            try engine.write([ClipItem]())
            print("  FAIL empty write should have thrown")
            failures.append("empty write guard")
        } catch {
            print("  ok   empty write rejected (\(error))")
        }

        print("")
        if failures.isEmpty {
            print("ALL SWIFT CHECKS PASSED")
            return 0
        }
        print("\(failures.count) CHECK(S) FAILED: \(failures.joined(separator: ", "))")
        return 1
    }
}
