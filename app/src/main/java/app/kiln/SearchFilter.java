package app.kiln;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Calendar;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/**
 * The extra conditions of an advanced search: extension, file type, size and date modified. The
 * file name itself is still the search box. {@link #compile()} turns the choices into a {@link Spec}
 * that answers "does this entry pass?" and can be sent to the Python search as JSON.
 */
final class SearchFilter {
    static final int ANY = 0, FOLDERS = 1, IMAGES = 2, VIDEO = 3, AUDIO = 4, DOCS = 5, TEXT = 6, ARCHIVES = 7;
    static final String[] TYPE_NAMES = {"Any", "Folders", "Images", "Video", "Audio", "Documents",
            "Text & code", "Archives"};

    static final int M_ANY = 0, M_TODAY = 1, M_WEEK = 2, M_MONTH = 3, M_YEAR = 4, M_OLD = 5;
    static final String[] MOD_NAMES = {"Any time", "Today", "Past 7 days", "Past 30 days", "Past year",
            "Older than a year"};

    private static final String IMAGE_EXT = "png jpg jpeg gif webp bmp heic heif svg tif tiff ico raw dng";
    private static final String VIDEO_EXT = "mp4 mkv webm 3gp mov avi m4v ts wmv flv mpg mpeg";
    private static final String AUDIO_EXT = "mp3 m4a flac ogg oga opus wav aac wma mka amr mid midi";
    private static final String DOC_EXT = "pdf doc docx odt rtf pages dot dotx xls xlsx xlsm ods csv tsv numbers "
            + "ppt pptx odp key pps ppsx epub mobi azw azw3 fb2 djvu";
    private static final String TEXT_EXT = "txt log md rst srt vtt nfo json xml yml yaml ini conf cfg toml "
            + "properties env html htm css js ts py c h cpp java kt gradle sh bash lua rb go rs sql php swift "
            + "dart jsx tsx vue cs scala mk cmake diff patch tex";
    private static final String ARCHIVE_EXT = "zip 7z rar tar gz tgz bz2 xz zst lz4 cab jar war z lzma kln";

    String exts = "", minText = "", maxText = "";
    int type = ANY, mod = M_ANY;

    void clear() {
        exts = "";
        minText = "";
        maxText = "";
        type = ANY;
        mod = M_ANY;
    }

    long minBytes() { return parseSize(minText); }

    long maxBytes() { return parseSize(maxText); }

    boolean active() { return count() > 0; }

    /** How many of the four conditions are switched on. */
    int count() {
        int n = 0;
        if (!parseExts(exts).isEmpty()) n++;
        if (type != ANY) n++;
        if (minBytes() >= 0 || maxBytes() >= 0) n++;
        if (mod != M_ANY) n++;
        return n;
    }

    /** One line saying what is switched on, e.g. "Images · .jpg .png · ≥ 10 MB · Past 7 days". */
    String summary() {
        List<String> p = new ArrayList<>();
        if (type != ANY) p.add(TYPE_NAMES[type]);
        List<String> e = parseExts(exts);
        if (!e.isEmpty()) {
            StringBuilder b = new StringBuilder();
            for (String x : e) b.append(b.length() == 0 ? "." : " .").append(x);
            p.add(b.toString());
        }
        long lo = minBytes(), hi = maxBytes();
        if (lo >= 0 && hi >= 0) p.add(Fmt.size(lo) + " \u2013 " + Fmt.size(hi));
        else if (lo >= 0) p.add("\u2265 " + Fmt.size(lo));
        else if (hi >= 0) p.add("\u2264 " + Fmt.size(hi));
        if (mod != M_ANY) p.add(MOD_NAMES[mod]);
        StringBuilder out = new StringBuilder();
        for (String s : p) out.append(out.length() == 0 ? "" : " \u00b7 ").append(s);
        return out.toString();
    }

    /** "jpg, .PNG;gif" -> [jpg, png, gif]. */
    static List<String> parseExts(String s) {
        List<String> out = new ArrayList<>();
        for (String w : s.toLowerCase(Locale.ROOT).split("[,;\\s]+")) {
            while (w.startsWith(".") || w.startsWith("*")) w = w.substring(1);
            if (!w.isEmpty() && !out.contains(w)) out.add(w);
        }
        return out;
    }

    /** "10", "10mb", "1.5 GB", "500k" -> bytes (no unit means MB); -1 when empty or not a size. */
    static long parseSize(String s) {
        String t = s.trim().toLowerCase(Locale.ROOT).replace(",", ".").replace(" ", "");
        if (t.isEmpty()) return -1;
        int i = 0;
        while (i < t.length() && (Character.isDigit(t.charAt(i)) || t.charAt(i) == '.')) i++;
        if (i == 0) return -1;
        double v;
        try {
            v = Double.parseDouble(t.substring(0, i));
        } catch (NumberFormatException e) {
            return -1;
        }
        String u = t.substring(i);
        if (u.endsWith("b") && u.length() > 1) u = u.substring(0, u.length() - 1);
        double mul;
        switch (u) {
            case "": case "m": case "mb": case "mib": mul = 1024.0 * 1024; break;
            case "b": mul = 1; break;
            case "k": case "kb": case "kib": mul = 1024; break;
            case "g": case "gb": case "gib": mul = 1024.0 * 1024 * 1024; break;
            case "t": case "tb": case "tib": mul = 1024.0 * 1024 * 1024 * 1024; break;
            default: return -1;
        }
        return (long) (v * mul);
    }

    private static Set<String> set(String exts) {
        Set<String> s = new HashSet<>();
        for (String e : exts.split("\\s+")) s.add(e);
        return s;
    }

    private static Set<String> typeSet(int type) {
        switch (type) {
            case IMAGES: return set(IMAGE_EXT);
            case VIDEO: return set(VIDEO_EXT);
            case AUDIO: return set(AUDIO_EXT);
            case DOCS: return set(DOC_EXT);
            case TEXT: return set(TEXT_EXT);
            case ARCHIVES: return set(ARCHIVE_EXT);
            default: return null;
        }
    }

    Spec compile() {
        Spec s = new Spec();
        List<String> typed = parseExts(exts);
        Set<String> tset = typeSet(type);
        long lo = minBytes(), hi = maxBytes();
        boolean sizeOn = lo >= 0 || hi >= 0;
        s.min = lo;
        s.max = hi;
        if (type == FOLDERS) {
            s.files = false;
            s.dirs = (!typed.isEmpty() || sizeOn) ? "none" : "only";
        } else {
            s.dirs = (!typed.isEmpty() || tset != null || sizeOn) ? "none" : "any";
            if (!typed.isEmpty()) {
                Set<String> e = new HashSet<>(typed);
                if (tset != null) e.retainAll(tset);
                if (e.isEmpty()) s.files = false;
                s.exts = e;
            } else if (tset != null) {
                s.exts = tset;
                s.arc = type == ARCHIVES;                    // also archives Kiln recognises by content
            }
        }
        long now = System.currentTimeMillis() / 1000;
        switch (mod) {
            case M_TODAY: {
                Calendar c = Calendar.getInstance();
                c.set(Calendar.HOUR_OF_DAY, 0);
                c.set(Calendar.MINUTE, 0);
                c.set(Calendar.SECOND, 0);
                c.set(Calendar.MILLISECOND, 0);
                s.after = c.getTimeInMillis() / 1000;
                break;
            }
            case M_WEEK: s.after = now - 7 * 86400L; break;
            case M_MONTH: s.after = now - 30 * 86400L; break;
            case M_YEAR: s.after = now - 365 * 86400L; break;
            case M_OLD: s.before = now - 365 * 86400L; break;
            default: break;
        }
        return s;
    }

    /** A compiled filter. Python's search reads the same fields from {@link #toJson()}. */
    static final class Spec {
        Set<String> exts;                // null = any extension
        boolean arc;                     // also accept files that are Kiln archives by content
        boolean files = true;            // false = no file can match
        String dirs = "any";             // any | only | none
        long min = -1, max = -1;         // bytes
        long after = -1, before = -1;    // epoch seconds; before is exclusive

        boolean accept(String name, boolean dir, long size, long mtime, boolean kln) {
            if (dir) return !dirs.equals("none") && dateOk(mtime);
            if (dirs.equals("only") || !files) return false;
            if (min >= 0 || max >= 0) {
                if (size < 0 || (min >= 0 && size < min) || (max >= 0 && size > max)) return false;
            }
            if (!dateOk(mtime)) return false;
            if (exts != null && !(arc && kln) && !exts.contains(Fmt.ext(name))) return false;
            return true;
        }

        private boolean dateOk(long m) {
            if (after < 0 && before < 0) return true;
            if (m <= 0) return false;
            return !(after >= 0 && m < after) && !(before >= 0 && m >= before);
        }

        String toJson() {
            try {
                JSONObject o = new JSONObject();
                if (exts != null) o.put("exts", new JSONArray(exts));
                o.put("arc", arc).put("files", files).put("dirs", dirs)
                        .put("min", min).put("max", max).put("after", after).put("before", before);
                return o.toString();
            } catch (JSONException e) {
                return "";
            }
        }
    }
}
