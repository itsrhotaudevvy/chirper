import qrcode from 'qrcode-generator';

/** A QR code as one SVG path of dark modules on a grid of `size` x `size` cells. */
export interface QrMatrix {
  size: number;
  d: string;
}

/** Encodes text as a QR code (error correction M). Throws when the text is too long for a QR code. */
export function qrMatrix(text: string): QrMatrix {
  const code = qrcode(0, 'M');
  code.addData(text, 'Byte');
  code.make();
  const size = code.getModuleCount();
  let d = '';
  for (let row = 0; row < size; row++) {
    for (let col = 0; col < size; col++) {
      if (code.isDark(row, col)) d += `M${col} ${row}h1v1h-1z`;
    }
  }
  return { size, d };
}
