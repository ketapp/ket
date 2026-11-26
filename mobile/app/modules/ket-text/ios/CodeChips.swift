// Code inside a line of text — a monospaced run on a fill, as the chat's
// markdown draws `code` — as rounded chips the height of its letters.
//
// React Native gives a nested <Text> no corners or padding. iOS fills its
// background square and as tall as the line, so code that wraps fuses into
// one block, and where a line breaks inside the code the fill runs on to the
// edge. React Native draws text with TextKit 1, whose layout manager hands
// each background it fills to `fillBackgroundRectArray`. That method is
// swapped here for one that draws a monospaced run's fill as a chip per line
// and leaves every other fill as it was.

import ObjectiveC
import UIKit

enum CodeChips {
  /// A chip's corners.
  static let radius: CGFloat = 4
  /// How far a chip reaches past its letters where a line breaks inside the
  /// code. At the code's two ends the text has room of its own: see
  /// `InlineCode` in src/components/ui/rows.tsx.
  static let reach: CGFloat = 3
  /// How much shorter a chip is than its font's line, top and bottom.
  static let inset: CGFloat = 0.5

  private static var installed = false

  static func install() {
    guard !installed else { return }
    installed = true
    swap(
      #selector(NSLayoutManager.drawBackground(forGlyphRange:at:)),
      #selector(NSLayoutManager.ket_drawBackground(forGlyphRange:at:)))
    swap(
      #selector(NSLayoutManager.fillBackgroundRectArray(_:count:forCharacterRange:color:)),
      #selector(NSLayoutManager.ket_fillBackgroundRectArray(_:count:forCharacterRange:color:)))
  }

  private static func swap(_ original: Selector, _ replacement: Selector) {
    guard let from = class_getInstanceMethod(NSLayoutManager.self, original),
      let to = class_getInstanceMethod(NSLayoutManager.self, replacement)
    else { return }
    method_exchangeImplementations(from, to)
  }

  /// Fills `range` as chips, a chip for each line it is on, if it is code;
  /// says whether it was. `origin` is where the text is drawn.
  static func fill(_ range: NSRange, color: UIColor, layout: NSLayoutManager, origin: CGPoint) -> Bool {
    guard let storage = layout.textStorage, NSMaxRange(range) <= storage.length,
      let context = UIGraphicsGetCurrentContext()
    else { return false }
    // The code's own face: its largest, as the room at its ends is set small.
    var face: UIFont?
    storage.enumerateAttribute(.font, in: range) { value, _, _ in
      if let font = value as? UIFont, font.pointSize > face?.pointSize ?? 0 { face = font }
    }
    guard let font = face, font.fontDescriptor.symbolicTraits.contains(.traitMonoSpace) else { return false }

    let text = storage.string as NSString
    let glyphs = layout.glyphRange(forCharacterRange: range, actualCharacterRange: nil)
    context.saveGState()
    context.setFillColor(color.cgColor)
    layout.enumerateLineFragments(forGlyphRange: glyphs) { line, _, container, lineGlyphs, _ in
      // This line's part of the code, without the space the line breaks at.
      var part = layout.characterRange(
        forGlyphRange: NSIntersectionRange(lineGlyphs, glyphs), actualGlyphRange: nil)
      while part.length > 0, isBreak(text.character(at: NSMaxRange(part) - 1)) { part.length -= 1 }
      while part.length > 0, isBreak(text.character(at: part.location)) {
        part.location += 1
        part.length -= 1
      }
      guard part.length > 0 else { return }
      let partGlyphs = layout.glyphRange(forCharacterRange: part, actualCharacterRange: nil)
      var chip = layout.boundingRect(forGlyphRange: partGlyphs, in: container)
      // Past the letters where the line breaks, within the text's bounds.
      if part.location > range.location { chip.origin.x -= reach; chip.size.width += reach }
      if NSMaxRange(part) < NSMaxRange(range) { chip.size.width += reach }
      let left = max(chip.minX, 0)
      chip.size.width = min(chip.maxX, container.size.width) - left
      chip.origin.x = left
      // From the face's ascender to its descender, on the line's baseline.
      let baseline = line.minY + layout.location(forGlyphAt: partGlyphs.location).y
      chip.origin.y = baseline - font.ascender + inset
      chip.size.height = font.ascender - font.descender - inset * 2
      let corner = min(radius, chip.width / 2, chip.height / 2)
      guard corner >= 0, chip.width > 0 else { return }
      context.addPath(
        CGPath(
          roundedRect: chip.offsetBy(dx: origin.x, dy: origin.y), cornerWidth: corner, cornerHeight: corner,
          transform: nil))
    }
    context.fillPath()
    context.restoreGState()
    return true
  }

  /// A space or a line's end: where a line can break inside code.
  private static func isBreak(_ unit: unichar) -> Bool {
    unit == 0x20 || unit == 0x0A || unit == 0x0D || unit == 0x09
  }
}

private var originKey: UInt8 = 0

extension NSLayoutManager {
  /// Notes where the text is drawn, for the fills drawn inside this call.
  @objc fileprivate func ket_drawBackground(forGlyphRange glyphs: NSRange, at origin: CGPoint) {
    objc_setAssociatedObject(self, &originKey, NSValue(cgPoint: origin), .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
    // Swapped: this is the layout manager's own.
    ket_drawBackground(forGlyphRange: glyphs, at: origin)
  }

  @objc fileprivate func ket_fillBackgroundRectArray(
    _ rects: UnsafePointer<CGRect>, count: Int, forCharacterRange range: NSRange, color: UIColor
  ) {
    if let origin = (objc_getAssociatedObject(self, &originKey) as? NSValue)?.cgPointValue,
      CodeChips.fill(range, color: color, layout: self, origin: origin)
    {
      return
    }
    ket_fillBackgroundRectArray(rects, count: count, forCharacterRange: range, color: color)
  }
}
