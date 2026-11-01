package app.kiln;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONException;

import java.util.ArrayList;
import java.util.List;
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

    // ---- animation speed: 0 Off, 1 Fast, 2 Normal (default, a touch quicker than before), 3 Relaxed

    static final String[] ANIM_NAMES = {"Off", "Fast", "Normal", "Relaxed"};
    private static final float[] ANIM_SCALE = {0f, 0.6f, 0.85f, 1.2f};

    static int animLevel(Context c) {
        return Math.max(0, Math.min(3, p(c).getInt("anim_level", 2)));
    }

    static void setAnimLevel(Context c, int v) { p(c).edit().putInt("anim_level", v).apply(); }

    /** A base duration in ms, scaled by the chosen speed. */
    static long anim(Context c, long ms) { return Math.round(ms * ANIM_SCALE[animLevel(c)]); }

    static boolean animOn(Context c) { return animLevel(c) > 0; }

    static boolean deepSearch(Context c) { return p(c).getBoolean("search_deep", true); }
    static void setDeepSearch(Context c, boolean v) { p(c).edit().putBoolean("search_deep", v).apply(); }

    // ---- bookmarks: folder paths, in the order they were added

    static List<String> bookmarks(Context c) {
        List<String> out = new ArrayList<>();
        try {
            JSONArray a = new JSONArray(p(c).getString("bookmarks", "[]"));
            for (int i = 0; i < a.length(); i++) {
                String s = a.optString(i, "");
                if (!s.isEmpty() && !out.contains(s)) out.add(s);
            }
        } catch (JSONException ignored) {
        }
        return out;
    }

    private static void saveBookmarks(Context c, List<String> l) {
        p(c).edit().putString("bookmarks", new JSONArray(l).toString()).apply();
    }

    static void addBookmark(Context c, String path) {
        List<String> l = bookmarks(c);
        if (!l.contains(path)) {
            l.add(path);
            saveBookmarks(c, l);
        }
    }

    static void removeBookmark(Context c, String path) {
        List<String> l = bookmarks(c);
        if (l.remove(path)) saveBookmarks(c, l);
    }

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
