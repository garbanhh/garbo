param([string]$In, [string]$Out)
# Convierte el PPM (P6) que vuelca GARBO_SCREENSHOT a PNG. Para Windows sin Python (en Linux,
# tools/ppm2png.py): powershell -File tools\ppm2png.ps1 -In captura.ppm -Out captura.png
Add-Type -AssemblyName System.Drawing
if (-not ("Ppm" -as [type])) {
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System; using System.IO; using System.Drawing; using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class Ppm {
    public static string Convert(string input, string output) {
        byte[] b = File.ReadAllBytes(input);
        int pos = 0; string[] f = new string[4];
        for (int i = 0; i < 4; i++) {
            int s = pos; while (b[pos] != 10 && b[pos] != 32) pos++;
            f[i] = System.Text.Encoding.ASCII.GetString(b, s, pos - s); pos++;
        }
        int w = int.Parse(f[1]), h = int.Parse(f[2]);
        using (var bmp = new Bitmap(w, h, PixelFormat.Format24bppRgb)) {
            var d = bmp.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.WriteOnly, bmp.PixelFormat);
            byte[] row = new byte[d.Stride];
            for (int y = 0; y < h; y++) {
                int src = pos + y * w * 3;
                for (int x = 0; x < w; x++) {
                    row[x * 3] = b[src + x * 3 + 2]; row[x * 3 + 1] = b[src + x * 3 + 1]; row[x * 3 + 2] = b[src + x * 3];
                }
                Marshal.Copy(row, 0, new IntPtr(d.Scan0.ToInt64() + (long)y * d.Stride), d.Stride);
            }
            bmp.UnlockBits(d);
            bmp.Save(output, ImageFormat.Png);
        }
        return w + "x" + h;
    }
}
'@
}
"$([Ppm]::Convert($In, $Out)) -> $Out"
