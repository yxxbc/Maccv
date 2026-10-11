//
//  PreviewRenderer.swift
//  MacCV
//
//  Renders the panel with fixed sample data, for the README image.
//
//  Why this instead of a screenshot: a screenshot has to be retaken by hand whenever
//  the UI changes, so it silently goes stale — the image in a README is usually one or
//  two releases behind, and nobody notices because nobody looks. Rendering the real
//  panel view from the real view code means `Scripts/render-preview.sh` regenerates it,
//  and what it shows cannot disagree with what the app draws.
//
//  It also keeps two things out of the repository that a screenshot would put there:
//  the developer's actual clipboard history, and their desktop wallpaper.
//
//  Invoked by `--render-preview`, which exits without starting the UI.
//

import AppKit
import SwiftUI

/// Builds the sample history shown in the preview.
///
/// The content is chosen to demonstrate what the app is *for*, not to look busy: a
/// rich-text paragraph, an image, a multi-file copy, and a masked API key. A screenshot
/// of five lines of prose does not tell a reader anything.
@MainActor
enum PreviewContent {
    /// Whether to build the sample content in Chinese.
    ///
    /// The sample text is localised with the interface: a Chinese screenshot of a panel
    /// full of English sentences shows the chrome translated and the content not, which
    /// is a worse advertisement than either language alone.
    static var useChinese: Bool = false

    private static func t(_ english: String, _ chinese: String) -> String {
        useChinese ? chinese : english
    }

    static func samples() -> [ClipSnapshot] {
        [
            snapshot(
                kind: .richText,
                representations: [
                    (
                        PasteboardUTI.html,
                        "<p>\(t("Q4 revenue is <b>up 18%</b> year over year.", "第四季度营收同比增长 <b>18%</b>。"))</p>"
                    ),
                    (PasteboardUTI.rtf, "{\\rtf1\\ansi Q4 revenue}"),
                    (
                        PasteboardUTI.string,
                        t("Q4 revenue is up 18% year over year.", "第四季度营收同比增长 18%。")
                    ),
                ],
                preview: t("Q4 revenue is up 18% year over year.", "第四季度营收同比增长 18%。"),
                byteLength: 4_820
            ),
            snapshot(
                kind: .image,
                representations: [
                    (PasteboardUTI.png, "fake-png-bytes-for-preview"),
                ],
                preview: "<image 1920×1080>",
                byteLength: 1_468_006
            ),
            snapshot(
                kind: .files,
                representations: [
                    (PasteboardUTI.fileURL, "file:///Documents/Invoice-2026-10.pdf"),
                    (PasteboardUTI.fileURL, "file:///Documents/Invoice-2026-09.pdf"),
                    (PasteboardUTI.fileURL, "file:///Documents/Receipts.zip"),
                ],
                preview: "Invoice-2026-10.pdf (+2)",
                byteLength: 214_388
            ),
            // Shows masking, which a still image otherwise cannot convey.
            snapshot(
                kind: .text,
                representations: [(PasteboardUTI.string, "sk_live_•••••••••••••••••••••")],
                preview: "sk_live_•••••••••••••••••••••",
                masked: [.apiKey],
                byteLength: 107
            ),
            snapshot(
                kind: .url,
                representations: [(PasteboardUTI.url, "https://yxxbc.github.io/Maccv/")],
                preview: "https://yxxbc.github.io/Maccv/",
                byteLength: 34
            ),
            snapshot(
                kind: .text,
                representations: [
                    (
                        PasteboardUTI.string,
                        t("Refactor the pasteboard snapshot path", "重构剪贴板快照的读取路径")
                    ),
                ],
                preview: t("Refactor the pasteboard snapshot path", "重构剪贴板快照的读取路径"),
                byteLength: 386
            ),
        ]
    }

    private static func snapshot(
        kind: ClipKind,
        representations: [(String, String)],
        preview: String,
        masked: [SensitiveKind] = [],
        byteLength: Int = 0
    ) -> ClipSnapshot {
        let items = representations.enumerated().map { index, pair in
            ClipItem(
                id: UUID(),
                kind: kind,
                preview: index == 0 ? preview : "",
                byteLength: index == 0 ? byteLength : 0,
                representations: [
                    ClipRepresentation(typeIdentifier: pair.0, data: Data(pair.1.utf8))
                ],
                maskedKinds: index == 0 ? masked : []
            )
        }
        // Summed rather than hard-coded: the row renders this value, so a sample that
        // disagrees with its own items shows a wrong size and looks like a bug in the app.
        return ClipSnapshot(
            changeCount: 1_234,
            byteLength: items.reduce(0) { $0 + $1.byteLength },
            items: items
        )
    }
}

/// Renders `ClipboardPanel` to a PNG over a generated backdrop.
@MainActor
enum PreviewRenderer {
    /// Renders the preview and writes it to `path`.
    ///
    /// - Parameters:
    ///   - path: where to write the PNG.
    ///   - language: the interface language, so both languages can be produced.
    /// Which appearance to render in. Dark matches the website and reads better over
    /// the generated backdrop.
    enum Appearance: String {
        case light
        case dark
    }

    /// How much the canvas is cropped around the panel.
    ///
    /// A README shows the image at full width, so the padded version reads well there. A
    /// hero image sits beside the headline at roughly half that, and at 60% padding the
    /// panel's 12pt rows shrink to about 6pt — legible in a design file, not on a screen.
    /// `tight` is for the website.
    enum Framing: String {
        case padded
        case tight
    }

    static func run(
        outputPath: String,
        language: String?,
        appearance: Appearance,
        framing: Framing = .padded
    ) -> Int32 {
        // Applying a language before the first accessor runs makes the catalog resolve
        // it for this process, which is the only way to render a specific language
        // without a relaunch.
        if let language {
            UserDefaults.standard.set([language], forKey: "AppleLanguages")
        }
        PreviewContent.useChinese = (language ?? "").hasPrefix("zh")

        let settings = AppSettings()
        // Masking on, so the preview shows the feature working.
        settings.maskSensitiveContent = true
        // The double-click hint in the footer mentions paste, which needs a permission
        // the renderer does not have; copying is what a reader will actually do.
        settings.rowDoubleClickAction = .copy

        let store = ClipboardStore()
        store.settings = settings
        store.loadSampleHistory(PreviewContent.samples())

        let updates = UpdateModel(settings: settings)

        let panel = ClipboardPanel()
            .environment(store)
            .environment(settings)
            .environment(updates)
            .frame(width: 380, height: 480)
            // The real panel inherits its background from the popover it lives in. Off
            // screen there is no host to inherit from, so without this the view draws
            // dark text on nothing and the result is unreadable — which is exactly what
            // the first attempt produced.
            .background(.regularMaterial)
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .environment(\.colorScheme, appearance == .dark ? .dark : .light)

        guard
            let panelImage = capture(
                panel,
                size: NSSize(width: 380, height: 480),
                appearance: appearance
            ),
            let composed = compose(panel: panelImage, framing: framing)
        else {
            FileHandle.standardError.write(Data("could not render the panel\n".utf8))
            return 1
        }

        guard
            let tiff = composed.tiffRepresentation,
            let rep = NSBitmapImageRep(data: tiff),
            let png = rep.representation(using: .png, properties: [:])
        else {
            FileHandle.standardError.write(Data("could not encode the PNG\n".utf8))
            return 1
        }

        do {
            try png.write(to: URL(fileURLWithPath: outputPath))
        } catch {
            FileHandle.standardError.write(Data("could not write \(outputPath): \(error)\n".utf8))
            return 1
        }

        print(
            "wrote \(outputPath) (\(Int(composed.size.width))×\(Int(composed.size.height)))"
        )
        return 0
    }

    /// Places the panel on a blurred, generated backdrop.
    ///
    /// The backdrop is drawn rather than taken from the screen on purpose. Using the
    /// real desktop wallpaper would commit a personal desktop to the repository, and
    /// shipping someone else's wallpaper would be a licensing problem. Generating it
    /// gives the same depth with no such cost.
    private static func compose(panel: NSImage, framing: Framing) -> NSImage? {
        let panelSize = panel.size

        // The canvas is a multiple of the panel's width, so the same number controls both
        // how much backdrop surrounds it and how large the panel appears when the image
        // is scaled to a fixed display width.
        let canvasSize: NSSize
        switch framing {
        case .padded:
            // 900/380 ≈ 2.4× — the panel occupies about 42% of the width, which reads well
            // at the full width a README gives it.
            canvasSize = NSSize(width: 900, height: 620)
        case .tight:
            // 1.55× — the panel occupies about two thirds, so at half the width its rows
            // stay near their designed size instead of halving into illegibility.
            canvasSize = NSSize(width: panelSize.width * 1.55, height: panelSize.height * 1.35)
        }

        guard let backdrop = capture(BackdropView(), size: canvasSize) else { return nil }

        let composed = NSImage(size: canvasSize)
        composed.lockFocus()
        defer { composed.unlockFocus() }

        backdrop.draw(in: NSRect(origin: .zero, size: canvasSize))

        // Centred horizontally, slightly above centre — a panel sitting exactly in the
        // middle reads as a product shot rather than as part of a desktop.
        let origin = NSPoint(
            x: (canvasSize.width - panelSize.width) / 2,
            y: (canvasSize.height - panelSize.height) / 2 + 14
        )

        // A soft drop shadow, so the panel separates from the backdrop the way a real
        // popover does.
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.45)
        shadow.shadowBlurRadius = 44
        shadow.shadowOffset = NSSize(width: 0, height: -12)
        shadow.set()
        NSColor(white: 0.08, alpha: 1).setFill()
        NSBezierPath(
            roundedRect: NSRect(origin: origin, size: panelSize), xRadius: 18, yRadius: 18
        ).fill()
        NSGraphicsContext.restoreGraphicsState()

        panel.draw(
            in: NSRect(origin: origin, size: panelSize),
            from: NSRect(origin: .zero, size: panelSize),
            operation: .sourceOver,
            fraction: 1
        )

        return composed
    }
}

/// A generated stand-in for a desktop wallpaper, blurred.
///
/// Deliberately abstract: soft colour fields rather than a picture. The point is to give
/// the panel context and depth, not to depict a specific desktop.
private struct BackdropView: View {
    var body: some View {
        ZStack {
            LinearGradient(
                colors: [
                    Color(red: 0.05, green: 0.05, blue: 0.09),
                    Color(red: 0.09, green: 0.07, blue: 0.16),
                    Color(red: 0.04, green: 0.05, blue: 0.10),
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )

            // Two soft shapes, so the blur has something to work with and the result
            // reads as depth rather than as a flat gradient.
            RadialGradient(
                colors: [
                    Color(red: 0.43, green: 0.29, blue: 0.91).opacity(0.55),
                    .clear,
                ],
                center: .init(x: 0.22, y: 0.78),
                startRadius: 0,
                endRadius: 420
            )

            RadialGradient(
                colors: [
                    Color(red: 0.13, green: 0.83, blue: 0.93).opacity(0.30),
                    .clear,
                ],
                center: .init(x: 0.82, y: 0.24),
                startRadius: 0,
                endRadius: 380
            )
        }
        .blur(radius: 34)
    }
}

extension PreviewRenderer {
    /// Captures a SwiftUI view by hosting it offscreen.
    ///
    /// `ImageRenderer` is the obvious tool and it does not work here: it produces a
    /// blank image for any view containing a `ScrollView`, because the scroll view needs
    /// a layout pass it never receives — verified with a minimal probe, where even a
    /// plain `VStack` in a `ScrollView` rendered empty. Hosting the view in an offscreen
    /// window and caching its display does perform layout, and it captures the real
    /// view rather than a re-creation of it.
    @MainActor
    static func capture(
        _ view: some View,
        size: NSSize,
        appearance: Appearance = .dark
    ) -> NSImage? {
        let hosting = NSHostingView(rootView: view.frame(width: size.width, height: size.height))
        hosting.frame = NSRect(origin: .zero, size: size)
        // The window's appearance is what the view reads for `.primary`/`.secondary`
        // colours, so it has to be set here rather than on the view alone.
        hosting.appearance = NSAppearance(
            named: appearance == .dark ? .darkAqua : .aqua
        )

        // A borderless window that is never shown: it exists to give the view a window
        // to lay out in. Showing it would flash a panel on screen during a build.
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.contentView = hosting
        window.isReleasedWhenClosed = false

        // Force the layout the ScrollView was waiting for.
        window.layoutIfNeeded()
        hosting.layoutSubtreeIfNeeded()

        guard let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            return nil
        }
        hosting.cacheDisplay(in: hosting.bounds, to: rep)

        let image = NSImage(size: size)
        image.addRepresentation(rep)
        return image
    }
}
