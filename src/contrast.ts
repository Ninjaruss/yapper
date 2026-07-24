/** WCAG 2.x relative-luminance contrast ratio between two #rrggbb colors.
 * Used only by the palette test — not shipped in any UI path. */
export function contrastRatio(fgHex: string, bgHex: string): number {
  const luminance = (hex: string): number => {
    const h = hex.replace("#", "");
    const channel = (i: number): number => {
      const c = parseInt(h.slice(i, i + 2), 16) / 255;
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
  };
  const [hi, lo] = [luminance(fgHex), luminance(bgHex)].sort((a, b) => b - a);
  return (hi + 0.05) / (lo + 0.05);
}
