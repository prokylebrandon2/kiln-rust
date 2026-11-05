package app.kiln;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.Iterator;
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

    // ---- elevated access to protected folders: 0 off, 1 root (su), 2 Shizuku

    static int privMode(Context c) { return Math.max(0, Math.min(2, p(c).getInt("priv_mode", 0))); }
    static void setPrivMode(Context c, int v) { p(c).edit().putInt("priv_mode", v).apply(); }

    // ---- Root only: allow delete, move, rename, create and write-back in protected folders (off by default)

    static boolean privUnsafe(Context c) { return p(c).getBoolean("priv_unsafe", false); }
    static void setPrivUnsafe(Context c, boolean v) { p(c).edit().putBoolean("priv_unsafe", v).apply(); }

    // ---- delete the original files once they are safely in the archive (off by default)

    static boolean deleteAfterPack(Context c) { return p(c).getBoolean("delete_after_pack", false); }
    static void setDeleteAfterPack(Context c, boolean v) {
        p(c).edit().putBoolean("delete_after_pack", v).apply();
    }

    // ---- what Delete does: 0 ask every time (default), 1 always to the recycle bin, 2 always for good

    static final String[] DELETE_NAMES = {"Ask", "Recycle bin", "Permanently"};

    static int deleteMode(Context c) { return Math.max(0, Math.min(2, p(c).getInt("delete_mode", 0))); }
    static void setDeleteMode(Context c, int v) { p(c).edit().putInt("delete_mode", v).apply(); }

    // ---- parallel jobs for packing (1 is safest inside an app)

    static int jobs(Context c) { return Math.max(1, Math.min(8, p(c).getInt("jobs", 1))); }
    static void setJobs(Context c, int v) { p(c).edit().putInt("jobs", v).apply(); }

    // ---- rows on screen: how many list rows fit one screen (16 is the default)

    static final int ROWS_MIN = 10, ROWS_MAX = 24;
    static final int ROWS_DEFAULT = 16;

    static int rowsOnScreen(Context c) { return Math.max(ROWS_MIN, Math.min(ROWS_MAX, p(c).getInt("rows_on_screen", ROWS_DEFAULT))); }

    static void setRowsOnScreen(Context c, int v) { p(c).edit().putInt("rows_on_screen", v).apply(); }

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

    // ---- how search results show their folder: 0 relative to where you are, 1 full path from /

    static final String[] PATH_NAMES = {"Relative", "Absolute"};

    static int searchPath(Context c) { return p(c).getInt("search_path", 0) == 1 ? 1 : 0; }
    static void setSearchPath(Context c, int v) { p(c).edit().putInt("search_path", v).apply(); }

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

    // ---- folder icons: the icon a person picked for a folder or bookmark (path -> icon key)

    private static Map<String, String> iconMap;

    private static synchronized Map<String, String> icons(Context c) {
        if (iconMap == null) {
            iconMap = new HashMap<>();
            try {
                JSONObject o = new JSONObject(p(c).getString("folder_icons", "{}"));
                Iterator<String> it = o.keys();
                while (it.hasNext()) {
                    String k = it.next();
                    String v = o.optString(k, "");
                    if (!v.isEmpty()) iconMap.put(k, v);
                }
            } catch (JSONException ignored) {
            }
        }
        return iconMap;
    }

    private static void saveIcons(Context c) {
        p(c).edit().putString("folder_icons", new JSONObject(icons(c)).toString()).apply();
    }

    /** The icon key picked for this folder, or null when it has none (it then gets its standard look). */
    static synchronized String folderIcon(Context c, String path) {
        return icons(c).get(path);
    }

    /** Remembers an icon for a folder; null or empty goes back to the standard look. */
    static synchronized void setFolderIcon(Context c, String path, String key) {
        if (key == null || key.isEmpty()) icons(c).remove(path); else icons(c).put(path, key);
        saveIcons(c);
    }

    // ---- folder colours: the colour a person picked for a folder's icon (path -> ARGB, absent = the normal accent)

    private static Map<String, Integer> colorMap;

    private static synchronized Map<String, Integer> colors(Context c) {
        if (colorMap == null) {
            colorMap = new HashMap<>();
            try {
                JSONObject o = new JSONObject(p(c).getString("folder_colors", "{}"));
                Iterator<String> it = o.keys();
                while (it.hasNext()) {
                    String k = it.next();
                    int v = o.optInt(k, 0);
                    if (v != 0) colorMap.put(k, v);
                }
            } catch (JSONException ignored) {
            }
        }
        return colorMap;
    }

    private static void saveColors(Context c) {
        p(c).edit().putString("folder_colors", new JSONObject(colors(c)).toString()).apply();
    }

    /** The colour (ARGB) picked for this folder, or 0 when it has none. */
    static synchronized int folderColor(Context c, String path) {
        Integer v = colors(c).get(path);
        return v == null ? 0 : v;
    }

    /** Remembers a colour for a folder; 0 goes back to the normal accent colour. */
    static synchronized void setFolderColor(Context c, String path, int color) {
        if (color == 0) colors(c).remove(path); else colors(c).put(path, color);
        saveColors(c);
    }

    /** The colour of a folder's icon in the list: its own pick, else the Settings default, else 0 (the normal accent). */
    static synchronized int folderColorOrDefault(Context c, String path) {
        int own = folderColor(c, path);
        return own != 0 ? own : defaultFolderColor(c);
    }

    /** The colour (ARGB) every folder gets unless it has its own, or 0 for the normal accent. Set in Settings. */
    static int defaultFolderColor(Context c) { return p(c).getInt("folder_color_default", 0); }

    static void setDefaultFolderColor(Context c, int color) {
        p(c).edit().putInt("folder_color_default", color).apply();
    }

    /** How many folders have a colour of their own. */
    static synchronized int customFolderColors(Context c) { return colors(c).size(); }

    /** Forgets every folder's own colour, so all folders follow the default. */
    static synchronized void clearFolderColors(Context c) {
        colors(c).clear();
        saveColors(c);
    }

    /** Changes whenever a folder colour or the default changes (the list redraws when it differs). */
    static synchronized int folderColorStamp(Context c) {
        int h = defaultFolderColor(c) * 31 + colors(c).size();
        for (Map.Entry<String, Integer> e : colors(c).entrySet()) h += e.getKey().hashCode() ^ e.getValue();
        return h;
    }

    /** A folder was renamed or moved: its icon (and its subfolders' icons) follow it. */
    static synchronized void moveFolderIcons(Context c, String from, String to) {
        Map<String, String> m = icons(c);
        Map<String, String> moved = new HashMap<>();
        for (Iterator<Map.Entry<String, String>> it = m.entrySet().iterator(); it.hasNext(); ) {
            Map.Entry<String, String> e = it.next();
            String k = e.getKey();
            if (k.equals(from) || k.startsWith(from + "/")) {
                moved.put(to + k.substring(from.length()), e.getValue());
                it.remove();
            }
        }
        if (!moved.isEmpty()) {
            m.putAll(moved);
            saveIcons(c);
        }
        Map<String, Integer> cm = colors(c);
        Map<String, Integer> cmoved = new HashMap<>();
        for (Iterator<Map.Entry<String, Integer>> it = cm.entrySet().iterator(); it.hasNext(); ) {
            Map.Entry<String, Integer> e = it.next();
            String k = e.getKey();
            if (k.equals(from) || k.startsWith(from + "/")) {
                cmoved.put(to + k.substring(from.length()), e.getValue());
                it.remove();
            }
        }
        if (cmoved.isEmpty()) return;
        cm.putAll(cmoved);
        saveColors(c);
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
