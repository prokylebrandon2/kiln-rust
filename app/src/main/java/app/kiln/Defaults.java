package app.kiln;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Map;
import java.util.TreeMap;

/**
 * "Always open .xyz files with ..." choices, plus the few on/off settings.
 * A default is stored per extension as "package|label", or "kiln:view" / "kiln:edit".
 */
final class Defaults {
    private Defaults() {}

    static final String KILN_VIEW = "kiln:view", KILN_EDIT = "kiln:edit";
    private static final String PREFIX = "def_";

    private static SharedPreferences p(Context c) {
        return c.getSharedPreferences("kiln", Context.MODE_PRIVATE);
    }

    // ---- switches

    static boolean upRow(Context c) { return p(c).getBoolean("up_row", true); }
    static boolean previews(Context c) { return p(c).getBoolean("previews", true); }
    static void setUpRow(Context c, boolean v) { p(c).edit().putBoolean("up_row", v).apply(); }
    static void setPreviews(Context c, boolean v) { p(c).edit().putBoolean("previews", v).apply(); }

    // ---- default apps

    /** The stored value for an extension, or null. */
    static String get(Context c, String ext) {
        if (ext == null || ext.isEmpty()) return null;
        return p(c).getString(PREFIX + ext, null);
    }

    static void set(Context c, String ext, String value) {
        if (ext == null || ext.isEmpty()) return;
        p(c).edit().putString(PREFIX + ext, value).apply();
    }

    static void clear(Context c, String ext) { p(c).edit().remove(PREFIX + ext).apply(); }

    static void clearAll(Context c) {
        SharedPreferences.Editor e = p(c).edit();
        for (String k : p(c).getAll().keySet()) {
            if (k.startsWith(PREFIX)) e.remove(k);
        }
        e.apply();
    }

    /** extension -> stored value, sorted by extension. */
    static Map<String, String> all(Context c) {
        Map<String, String> out = new TreeMap<>();
        for (Map.Entry<String, ?> en : p(c).getAll().entrySet()) {
            if (en.getKey().startsWith(PREFIX) && en.getValue() instanceof String) {
                out.put(en.getKey().substring(PREFIX.length()), (String) en.getValue());
            }
        }
        return out;
    }

    static String pkgOf(String v) {
        int i = v.indexOf('|');
        return i < 0 ? v : v.substring(0, i);
    }

    static String labelOf(String v) {
        if (KILN_VIEW.equals(v)) return "Kiln (view)";
        if (KILN_EDIT.equals(v)) return "Kiln (edit)";
        int i = v.indexOf('|');
        return i < 0 ? v : v.substring(i + 1);
    }
}
