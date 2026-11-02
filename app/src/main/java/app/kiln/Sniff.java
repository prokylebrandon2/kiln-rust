package app.kiln;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CoderResult;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;

/**
 * Looks inside a file whose name doesn't say what it is (no extension, or one Kiln doesn't know):
 * first for well-known signatures, then for whether the bytes read as text. Reads at most the
 * first 8 KB, and only when the person taps the file, never while a folder is being listed.
 */
final class Sniff {
    private Sniff() {}

    /** Result of {@link #kind}: text Kiln can show, an image Kiln can show, or neither. */
    static final int TEXT = 0, IMAGE = 2, BINARY = -1;

    private static final int HEAD = 8192;

    static byte[] head(File f) {
        byte[] b = new byte[HEAD];
        try (FileInputStream in = new FileInputStream(f)) {
            int n = 0;
            while (n < b.length) {
                int got = in.read(b, n, b.length - n);
                if (got < 0) break;
                n += got;
            }
            return n == b.length ? b : java.util.Arrays.copyOf(b, n);
        } catch (IOException e) {
            return null;
        }
    }

    /** TEXT (including an empty file), IMAGE (png, jpeg, gif, webp, bmp) or BINARY. */
    static int kind(File f) {
        byte[] b = head(f);
        if (b == null) return BINARY;
        if (b.length == 0) return TEXT;
        String m = magicMime(b);
        if (m != null) return m.startsWith("image/") && isViewableImage(m) ? IMAGE : BINARY;
        return looksText(b) ? TEXT : BINARY;
    }

    /** The best MIME type to hand to other apps; "*&#47;*" when nothing is known. */
    static String mime(File f) {
        byte[] b = head(f);
        if (b == null) return "*/*";
        if (b.length == 0) return "text/plain";
        String m = magicMime(b);
        if (m != null) return m;
        return looksText(b) ? "text/plain" : "*/*";
    }

    private static boolean isViewableImage(String m) {
        return m.equals("image/png") || m.equals("image/jpeg") || m.equals("image/gif")
                || m.equals("image/webp") || m.equals("image/bmp");
    }

    private static boolean at(byte[] b, int off, int... v) {
        if (b.length < off + v.length) return false;
        for (int i = 0; i < v.length; i++) if ((b[off + i] & 0xFF) != v[i]) return false;
        return true;
    }

    private static boolean ascii(byte[] b, int off, String s) {
        if (b.length < off + s.length()) return false;
        for (int i = 0; i < s.length(); i++) if (b[off + i] != (byte) s.charAt(i)) return false;
        return true;
    }

    /** MIME type from a well-known signature, or null. */
    static String magicMime(byte[] b) {
        if (at(b, 0, 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)) return "image/png";
        if (at(b, 0, 0xFF, 0xD8, 0xFF)) return "image/jpeg";
        if (ascii(b, 0, "GIF87a") || ascii(b, 0, "GIF89a")) return "image/gif";
        if (ascii(b, 0, "RIFF") && ascii(b, 8, "WEBP")) return "image/webp";
        if (ascii(b, 0, "RIFF") && ascii(b, 8, "WAVE")) return "audio/x-wav";
        if (ascii(b, 0, "BM") && b.length > 14 && at(b, 6, 0, 0, 0, 0)) return "image/bmp";
        if (ascii(b, 0, "%PDF-")) return "application/pdf";
        if (ascii(b, 0, "PK") && (at(b, 2, 3, 4) || at(b, 2, 5, 6))) return "application/zip";
        if (at(b, 0, 0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C)) return "application/x-7z-compressed";
        if (at(b, 0, 0x1F, 0x8B)) return "application/gzip";
        if (ascii(b, 0, "Rar!")) return "application/vnd.rar";
        if (ascii(b, 0, "OggS")) return "audio/ogg";
        if (ascii(b, 0, "fLaC")) return "audio/flac";
        if (ascii(b, 0, "ID3") || at(b, 0, 0xFF, 0xFB) || at(b, 0, 0xFF, 0xFA) || at(b, 0, 0xFF, 0xF3)
                || at(b, 0, 0xFF, 0xF2)) return "audio/mpeg";
        if (ascii(b, 4, "ftyp")) return "video/mp4";
        if (at(b, 0, 0x1A, 0x45, 0xDF, 0xA3)) return "video/x-matroska";
        if (ascii(b, 0, "SQLite format 3")) return "application/vnd.sqlite3";
        if (at(b, 0, 0x7F, 0x45, 0x4C, 0x46)) return "application/x-executable";
        if (ascii(b, 0, "dex\n")) return "application/octet-stream";
        if (ascii(b, 0, "KLNA") || ascii(b, 0, "KLN9") || ascii(b, 0, "KLN2")) return "application/octet-stream";
        return null;
    }

    /** True when the bytes read as text: no NUL, few control characters and, if there are
     *  non-ASCII bytes, valid UTF-8 (a character cut off at the end of the sample is fine). */
    static boolean looksText(byte[] b) {
        int ctl = 0;
        for (byte x : b) {
            int v = x & 0xFF;
            if (v == 0) return false;
            if (v < 32 && v != '\n' && v != '\r' && v != '\t' && v != '\f' && v != 27 && v != 8) ctl++;
        }
        if (ctl * 50 > b.length) return false;            // more than 2% control characters
        CharsetDecoder d = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT);
        CoderResult r = d.decode(ByteBuffer.wrap(b), CharBuffer.allocate(b.length + 1), false);
        return !r.isError();
    }
}
