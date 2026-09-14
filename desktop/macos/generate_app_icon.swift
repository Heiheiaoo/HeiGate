#!/usr/bin/env swift
import Cocoa

/// Generates macOS standard squircle AppIcon with white gradient background and shadow,
/// matching macOS Big Sur / Monterey / Ventura / Sonoma / Sequoia HIG specifications.

func renderMasterIcon(svgPath: String) -> NSImage {
    let canvasSize = NSSize(width: 1024, height: 1024)
    let image = NSImage(size: canvasSize)

    image.lockFocus()

    guard let ctx = NSGraphicsContext.current?.cgContext else {
        fatalError("No CGContext available")
    }

    ctx.clear(CGRect(origin: .zero, size: canvasSize))

    // macOS standard squircle geometry (824x824 at (100, 96))
    let squircleRect = NSRect(x: 100, y: 96, width: 824, height: 824)
    let cornerRadius: CGFloat = 185.0
    let squirclePath = NSBezierPath(roundedRect: squircleRect, xRadius: cornerRadius, yRadius: cornerRadius)

    // Deep ambient drop shadow
    ctx.saveGState()
    let shadowColor = NSColor(red: 0, green: 0, blue: 0, alpha: 0.22).cgColor
    ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 28, color: shadowColor)
    NSColor.white.setFill()
    squirclePath.fill()
    ctx.restoreGState()

    // Crisp contact shadow
    ctx.saveGState()
    let contactColor = NSColor(red: 0, green: 0, blue: 0, alpha: 0.12).cgColor
    ctx.setShadow(offset: CGSize(width: 0, height: -4), blur: 10, color: contactColor)
    NSColor.white.setFill()
    squirclePath.fill()
    ctx.restoreGState()

    // Clip to squircle for surface drawing
    ctx.saveGState()
    squirclePath.addClip()

    // Apple-style white gradient: pure clean white top to very subtle platinum white bottom
    let topWhite = NSColor(red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0)
    let bottomWhite = NSColor(red: 0.955, green: 0.965, blue: 0.978, alpha: 1.0)
    let bgGradient = NSGradient(starting: bottomWhite, ending: topWhite)!
    bgGradient.draw(in: squircleRect, angle: 90)

    // Subtle inner highlight ring (simulates light bevel)
    let innerHighlightRect = squircleRect.insetBy(dx: 1.5, dy: 1.5)
    let innerHighlightPath = NSBezierPath(roundedRect: innerHighlightRect, xRadius: cornerRadius - 1.5, yRadius: cornerRadius - 1.5)
    let highlightGradient = NSGradient(
        starting: NSColor(white: 1.0, alpha: 0.2),
        ending: NSColor(white: 1.0, alpha: 0.95)
    )!
    ctx.saveGState()
    innerHighlightPath.lineWidth = 1.5
    highlightGradient.draw(in: innerHighlightPath, angle: 90)
    ctx.restoreGState()

    // Outer subtle hairline border for edge contrast on bright docks
    let hairlineBorder = NSColor(red: 0.08, green: 0.12, blue: 0.20, alpha: 0.08)
    hairlineBorder.setStroke()
    squirclePath.lineWidth = 1.5
    squirclePath.stroke()

    // Load and draw SVG logo with optical centering
    let svgURL = URL(fileURLWithPath: svgPath)
    if let svgImage = NSImage(contentsOf: svgURL) {
        let logoSize: CGFloat = 680.0
        let offsetX = -((268.0 - 256.0) / 512.0) * logoSize
        let shiftY = -((14.0) / 512.0) * logoSize
        
        let squircleCenterX = squircleRect.midX
        let squircleCenterY = squircleRect.midY
        
        let logoRect = NSRect(
            x: squircleCenterX - logoSize / 2.0 + offsetX,
            y: squircleCenterY - logoSize / 2.0 + shiftY,
            width: logoSize,
            height: logoSize
        )
        svgImage.draw(in: logoRect, from: NSRect(origin: .zero, size: svgImage.size), operation: .sourceOver, fraction: 1.0)
    }

    ctx.restoreGState()
    image.unlockFocus()
    return image
}

func resizeAndSave(image: NSImage, targetPixelSize: Int, destURL: URL) {
    let size = NSSize(width: targetPixelSize, height: targetPixelSize)
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: targetPixelSize,
        pixelsHigh: targetPixelSize,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ) else {
        fatalError("Could not create NSBitmapImageRep for size \(targetPixelSize)")
    }
    
    rep.size = size
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    
    image.draw(in: NSRect(origin: .zero, size: size),
               from: NSRect(origin: .zero, size: image.size),
               operation: .copy,
               fraction: 1.0)
    
    NSGraphicsContext.restoreGraphicsState()
    
    if let pngData = rep.representation(using: .png, properties: [:]) {
        try? pngData.write(to: destURL)
    }
}

let scriptDir = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().path
let rootDir = (scriptDir as NSString).deletingLastPathComponent
let svgPath = "\(rootDir)/frontend/public/logo.svg"
let iconsetPath = "\(scriptDir)/AppIcon.iconset"

print("Rendering master icon from \(svgPath)...")
let master = renderMasterIcon(svgPath: svgPath)

let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024)
]

for (filename, pixelSize) in sizes {
    let fileURL = URL(fileURLWithPath: "\(iconsetPath)/\(filename)")
    resizeAndSave(image: master, targetPixelSize: pixelSize, destURL: fileURL)
}

print("All iconset PNGs generated into \(iconsetPath)")
