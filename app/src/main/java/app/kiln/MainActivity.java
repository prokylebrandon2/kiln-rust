package app.kiln;

import android.Manifest;
import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.content.res.ColorStateList;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.MimeTypeMap;
import android.widget.ArrayAdapter;
import android.widget.BaseAdapter;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.ProgressBar;
import android.widget.TextView;
import android.widget.Toast;

import androidx.core.content.FileProvider;

import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import com.chaquo.python.android.AndroidPlatform;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/** A file manager that also understands Kiln archives. */
public class MainActivity extends Activity {

    // ------------------------------------------------------------------ types

    /** Python reports progress here and polls for cancellation. Public so Python can call it. */
    public static class Progress {
        private final MainActivity owner;
        private volatile boolean cancel;

        Progress(MainActivity owner) { this.owner = owner; }

        public void event(String json) { owner.onProgress(json); }

        public boolean cancelled() { return cancel; }

        void cancel() { cancel = true; }
    }

    interface Job { JSONObject run(Progress p) throws Exception; }
    interface Done { void done(JSONObject result); }
    interface OnFile { void got(File f); }
    interface OnText { void got(String s); }
    /** Returns an error message to show under the field, or null when the name was accepted. */
    interface OnName { String got(String name); }

    static final class Item { String name, kind; long size, mtime; int count; boolean enc; }
    static final class Ent { String p; long s, m; }
    static final class Row {
        String key, title, sub, right, ext = "";
        int icon;
        boolean dir, folder;
        long size, time;
        Item item;
        Ent ent;
    }
    static final class Arc {
        File file;
        String password = "";
        boolean isSet, encrypted;
        long packed;
        String prefix = "";
        final List<Ent> ents = new ArrayList<>();
    }

    private static final int S_NAME = 0, S_SIZE = 1, S_DATE = 2, S_TYPE = 3;
    private static final int M_ALL_FILES = 1, M_HIDDEN = 2, M_REFRESH = 3, M_ABOUT = 4,
            M_SELECT_ALL = 5, M_EXTRACT = 6, M_RENAME = 7, M_CLEAR = 9, M_TEST = 10,
            M_INFO = 11, M_DECRYPT = 12, M_EXTRACT_ALL = 13, M_PLAIN = 14;
    private static final long IMAGE_PREVIEW_MAX = 15_000_000L;
    private static final String TEXT_EXT = " txt md json xml csv log py c h cpp java kt sh html css js "
            + "yml yaml ini conf cfg toml tsv rst srt gradle properties lua rb go rs ts sql "
            + "bash zsh fish bat cmd ps1 pl php swift dart jsx tsx vue cs scala mk cmake env "
            + "gitignore service desktop lock diff patch tex ";
    private static final String IMAGE_EXT = " png jpg jpeg gif webp bmp ";

    // ------------------------------------------------------------------ state

    private final Handler ui = new Handler(Looper.getMainLooper());
    private final File root = Environment.getExternalStorageDirectory();
    private final List<Row> rows = new ArrayList<>();
    private final List<Item> items = new ArrayList<>();
    private final LinkedHashSet<String> sel = new LinkedHashSet<>();
    private final List<File> clip = new ArrayList<>();
    private boolean clipMove;
    private volatile PyObject app;           // kiln_app module; null until Python is up
    private volatile boolean svcOn = false;
    private SharedPreferences prefs;
    private int sortKey = S_NAME;
    private boolean sortAsc = true;
    private boolean showAll = true;
    private boolean showHidden = false;
    private boolean plainSet = false;        // viewing a random-name set as an ordinary folder
    private boolean loadedOnce = false;
    private boolean askedNotif = false;
    private boolean needRefresh = false;
    private File cwd = Environment.getExternalStorageDirectory();
    private Arc arc;                         // non-null while an archive is open
    private ProgressUi currentProgress;

    private ImageButton left, sortBtn, more;
    private TextView title, sub, empty, banner;
    private ListView list;
    private View permView;
    private Button fab;
    private LinearLayout bottomBar;
    private RowAdapter adapter;

    // ------------------------------------------------------------------ lifecycle

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = getSharedPreferences("kiln", MODE_PRIVATE);
        sortKey = prefs.getInt("sort_key", S_NAME);
        sortAsc = prefs.getBoolean("sort_asc", true);
        showAll = prefs.getBoolean("show_all", true);
        showHidden = prefs.getBoolean("show_hidden", false);
        deleteTree(new File(getCacheDir(), "preview"));
        buildUi();
        startPython();
    }

    @Override
    protected void onResume() {
        super.onResume();
        refreshPermission();
        if (needRefresh && arc == null && loadedOnce) {
            needRefresh = false;
            loadDir(cwd, true);
        }
    }

    @Override
    public void onBackPressed() {
        if (!sel.isEmpty()) {
            sel.clear();
            redraw();
        } else if (arc != null) {
            if (arc.prefix.isEmpty()) {
                closeArchive();
            } else {
                String p = arc.prefix.substring(0, arc.prefix.length() - 1);
                int i = p.lastIndexOf('/');
                arc.prefix = i < 0 ? "" : p.substring(0, i + 1);
                buildArcRows();
            }
        } else if (!cwd.equals(root) && cwd.getParentFile() != null) {
            loadDir(cwd.getParentFile());
        } else {
            super.onBackPressed();
        }
    }

    // ------------------------------------------------------------------ small helpers

    private int dp(int v) { return Ui.dp(this, v); }

    private void toast(String s) { Toast.makeText(this, s, Toast.LENGTH_SHORT).show(); }

    private void message(String heading, String body) {
        new Sheet(this, heading).text(body).button("OK", Ui.FILLED, null).show();
    }

    private void confirm(String heading, String body, String yes, int kind, final Runnable action) {
        new Sheet(this, heading).text(body)
                .button("Cancel", Ui.TEXT, null)
                .button(yes, kind, () -> {
                    action.run();
                    return true;
                }).show();
    }

    private static JSONObject errObj(String msg) {
        JSONObject o = new JSONObject();
        try {
            o.put("ok", false);
            o.put("error", msg);
            o.put("message", msg);
        } catch (JSONException ignored) {
        }
        return o;
    }

    private JSONObject call(String fn, Object... args) throws Exception {
        PyObject m = app;
        if (m == null) throw new IllegalStateException("Python is still starting");
        return new JSONObject(m.callAttr(fn, args).toString());
    }

    /** Runs quick work off the UI thread, without a progress box. */
    private void bg(final Job job, final Done done) {
        new Thread(() -> {
            JSONObject r;
            try {
                r = job.run(null);
            } catch (Throwable t) {
                r = errObj(String.valueOf(t.getMessage()));
            }
            final JSONObject res = r;
            ui.post(() -> done.done(res));
        }).start();
    }

    private static boolean deleteTree(File f) {
        if (f.isDirectory()) {
            File[] kids = f.listFiles();
            if (kids != null) {
                for (File k : kids) deleteTree(k);
            }
        }
        return f.delete();
    }

    private String pathsJson(List<File> files) {
        JSONArray a = new JSONArray();
        for (File f : files) a.put(f.getPath());
        return a.toString();
    }

    private static int kindOf(String name) {
        String ext = " " + Fmt.ext(name) + " ";
        if (TEXT_EXT.contains(ext)) return 1;
        if (IMAGE_EXT.contains(ext)) return 2;
        return 0;
    }

    private static String plural(long n, String one, String many) {
        return n + " " + (n == 1 ? one : many);
    }

    // ------------------------------------------------------------------ layout

    private void buildUi() {
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setBackgroundColor(Ui.bg(this));

        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(dp(4), dp(6), dp(4), dp(6));
        left = Ui.iconButton(this, R.drawable.ic_up, v -> onLeft());
        bar.addView(left, new LinearLayout.LayoutParams(dp(48), dp(48)));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        title = Ui.label(this, "Kiln", 19, true, false);
        title.setSingleLine(true);
        title.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        sub = Ui.label(this, "", 12, false, true);
        sub.setSingleLine(true);
        sub.setEllipsize(TextUtils.TruncateAt.START);
        titles.addView(title);
        titles.addView(sub);
        bar.addView(titles, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        sortBtn = Ui.iconButton(this, R.drawable.ic_sort, v -> sortSheet());
        bar.addView(sortBtn, new LinearLayout.LayoutParams(dp(48), dp(48)));
        more = Ui.iconButton(this, R.drawable.ic_more, v -> showMenu());
        bar.addView(more, new LinearLayout.LayoutParams(dp(48), dp(48)));
        page.addView(bar);

        View line = new View(this);
        line.setBackgroundColor(Ui.line(this));
        page.addView(line, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(1)));

        banner = Ui.label(this, "", 13, false, false);
        banner.setPadding(dp(16), dp(10), dp(16), dp(10));
        banner.setBackgroundColor(getColor(R.color.k_warn_bg));
        banner.setVisibility(View.GONE);
        page.addView(banner);

        FrameLayout body = new FrameLayout(this);
        list = new ListView(this);
        list.setDivider(null);
        list.setSelector(android.R.color.transparent);
        adapter = new RowAdapter();
        list.setAdapter(adapter);
        list.setOnItemClickListener((av, v, pos, id) -> onRowClick(pos));
        list.setOnItemLongClickListener((av, v, pos, id) -> {
            toggle(rows.get(pos).key);
            return true;
        });
        body.addView(list, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        empty = Ui.label(this, "Starting...", 15, false, true);
        empty.setGravity(Gravity.CENTER);
        empty.setPadding(dp(32), dp(32), dp(32), dp(32));
        body.addView(empty, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        list.setEmptyView(empty);

        fab = Ui.button(this, "+  New", Ui.FILLED, v -> newSheet());
        FrameLayout.LayoutParams fp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, dp(48), Gravity.BOTTOM | Gravity.END);
        fp.setMargins(0, 0, dp(16), dp(16));
        body.addView(fab, fp);

        LinearLayout perm = new LinearLayout(this);
        perm.setOrientation(LinearLayout.VERTICAL);
        perm.setGravity(Gravity.CENTER);
        perm.setPadding(dp(32), dp(32), dp(32), dp(32));
        TextView why = Ui.label(this, "Kiln needs access to your files to open, create and unpack "
                + "archives and to manage files. Nothing leaves your phone.", 15, false, false);
        why.setGravity(Gravity.CENTER);
        perm.addView(why);
        LinearLayout.LayoutParams gp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        gp.topMargin = dp(16);
        perm.addView(Ui.button(this, "Allow access to files", Ui.FILLED, v -> askStorage()), gp);
        permView = perm;
        permView.setVisibility(View.GONE);
        permView.setBackgroundColor(Ui.bg(this));            // opaque so it covers the list
        body.addView(permView, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        page.addView(body, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        bottomBar = new LinearLayout(this);
        bottomBar.setOrientation(LinearLayout.VERTICAL);
        bottomBar.setBackgroundColor(Ui.surface(this));
        bottomBar.setVisibility(View.GONE);
        page.addView(bottomBar, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        setContentView(page);
        renderHeader();
    }

    /** One row: icon, name and date, size on the right. A selected row is tinted. */
    private final class RowView extends LinearLayout {
        final ImageView icon;
        final TextView name, detail, size;

        RowView() {
            super(MainActivity.this);
            setOrientation(HORIZONTAL);
            setGravity(Gravity.CENTER_VERTICAL);
            setPadding(dp(16), dp(11), dp(16), dp(11));
            icon = new ImageView(MainActivity.this);
            addView(icon, new LayoutParams(dp(28), dp(28)));
            LinearLayout col = new LinearLayout(MainActivity.this);
            col.setOrientation(VERTICAL);
            name = Ui.label(MainActivity.this, "", 16, false, false);
            name.setSingleLine(true);
            name.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            detail = Ui.label(MainActivity.this, "", 12, false, true);
            detail.setSingleLine(true);
            col.addView(name);
            col.addView(detail);
            LayoutParams cp = new LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            cp.leftMargin = dp(16);
            addView(col, cp);
            size = Ui.label(MainActivity.this, "", 13, false, true);
            size.setSingleLine(true);
            LayoutParams sp = new LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT);
            sp.leftMargin = dp(12);
            addView(size, sp);
        }

        void bind(Row r, boolean selected) {
            if (selected) {
                icon.setImageResource(R.drawable.ic_check);
                Ui.tint(icon, Ui.accent(getContext()));
            } else {
                icon.setImageResource(r.icon);
                Ui.tint(icon, r.folder || r.dir ? Ui.accent(getContext()) : Ui.text2(getContext()));
            }
            name.setText(r.title);
            detail.setText(r.sub);
            detail.setVisibility(r.sub.isEmpty() ? View.GONE : View.VISIBLE);
            size.setText(r.right);
            setBackgroundColor(selected ? getColor(R.color.k_sel) : 0);
        }
    }

    private final class RowAdapter extends BaseAdapter {
        @Override public int getCount() { return rows.size(); }
        @Override public Object getItem(int i) { return rows.get(i); }
        @Override public long getItemId(int i) { return i; }

        @Override
        public View getView(int i, View convert, ViewGroup parent) {
            RowView v = convert instanceof RowView ? (RowView) convert : new RowView();
            Row r = rows.get(i);
            v.bind(r, sel.contains(r.key));
            return v;
        }
    }

    // ------------------------------------------------------------------ header, bars, menus

    private void redraw() {
        adapter.notifyDataSetChanged();
        renderHeader();
    }

    private void renderHeader() {
        boolean selecting = !sel.isEmpty();
        left.setVisibility(View.VISIBLE);
        sortBtn.setVisibility(selecting ? View.GONE : View.VISIBLE);
        if (selecting) {
            left.setImageResource(R.drawable.ic_close);
            title.setText(sel.size() + " selected");
            sub.setText("");
        } else if (arc != null) {
            left.setImageResource(R.drawable.ic_up);
            title.setText(arc.file.getName());
            sub.setText(arc.prefix.isEmpty()
                    ? plural(arc.ents.size(), "file", "files") + (arc.encrypted ? " \u00b7 encrypted" : "")
                    : "/" + arc.prefix);
        } else {
            left.setImageResource(R.drawable.ic_up);
            boolean atRoot = cwd.equals(root);
            left.setVisibility(atRoot ? View.INVISIBLE : View.VISIBLE);
            title.setText(atRoot ? "Kiln" : cwd.getName());
            sub.setText(cwd.getPath());
        }
        banner.setVisibility(plainSet && arc == null ? View.VISIBLE : View.GONE);
        banner.setText("Plain view of an encrypted set. Renaming or deleting files here can make "
                + "the set unreadable.");
        renderBars();
    }

    private void renderBars() {
        bottomBar.removeAllViews();
        boolean selecting = !sel.isEmpty();
        boolean show = true;
        if (arc != null) {
            if (selecting) {
                bar(btn("Extract", Ui.FILLED, this::extractSelected));
            } else {
                bar(btn("Extract all", Ui.FILLED, () -> extractDialog(new ArrayList<String>())));
            }
        } else if (selecting) {
            bar(btn("Copy", Ui.OUTLINE, () -> setClip(false)),
                    btn("Move", Ui.OUTLINE, () -> setClip(true)),
                    btn("Delete", Ui.DANGER, this::deleteSelected),
                    btn("Pack", Ui.FILLED, () -> packDialog(selectedFiles())));
        } else if (!clip.isEmpty()) {
            TextView t = Ui.label(this, (clipMove ? "Move " : "Copy ")
                    + plural(clip.size(), "item", "items") + " here?", 13, false, true);
            t.setPadding(dp(16), dp(10), dp(16), 0);
            bottomBar.addView(t);
            bar(btn("Cancel", Ui.TEXT, () -> {
                clip.clear();
                redraw();
            }), btn(clipMove ? "Move here" : "Paste here", Ui.FILLED, this::paste));
        } else {
            show = false;
        }
        bottomBar.setVisibility(show ? View.VISIBLE : View.GONE);
        boolean fabOn = arc == null && !selecting && clip.isEmpty() && app != null && hasStorage();
        fab.setVisibility(fabOn ? View.VISIBLE : View.GONE);
    }

    private Button btn(String label, int kind, final Runnable r) {
        return Ui.button(this, label, kind, v -> r.run());
    }

    private void bar(Button... buttons) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setPadding(dp(12), dp(10), dp(12), dp(10));
        for (int i = 0; i < buttons.length; i++) {
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0,
                    ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            if (i > 0) lp.leftMargin = dp(8);
            buttons[i].setMinimumWidth(0);
            row.addView(buttons[i], lp);
        }
        bottomBar.addView(row);
    }

    private void onLeft() {
        if (!sel.isEmpty()) {
            sel.clear();
            redraw();
        } else {
            onBackPressed();
        }
    }

    /** A bottom-sheet menu: labels and the ids onMenu() understands. */
    private void menuSheet(String heading, String[] labels, final int[] ids) {
        Sheet s = new Sheet(this, heading);
        for (int i = 0; i < labels.length; i++) {
            final int id = ids[i];
            s.option(labels[i], false, () -> onMenu(id));
        }
        s.show();
    }

    private void showMenu() {
        List<String> l = new ArrayList<>();
        List<Integer> id = new ArrayList<>();
        boolean selecting = !sel.isEmpty();
        if (arc != null) {
            if (selecting) {
                l.add("Select all"); id.add(M_SELECT_ALL);
                l.add("Clear selection"); id.add(M_CLEAR);
            } else {
                l.add("Extract all..."); id.add(M_EXTRACT_ALL);
                l.add("Check integrity"); id.add(M_TEST);
                l.add("Select all"); id.add(M_SELECT_ALL);
                l.add("Archive info"); id.add(M_INFO);
                if (arc.isSet) {
                    l.add("Browse as plain folder"); id.add(M_PLAIN);
                    l.add("Decrypt folder in place..."); id.add(M_DECRYPT);
                }
            }
        } else if (selecting) {
            l.add("Select all"); id.add(M_SELECT_ALL);
            if (sel.size() == 1) { l.add("Rename"); id.add(M_RENAME); }
            if (sel.size() == 1 && selectedArchive() != null) { l.add("Extract..."); id.add(M_EXTRACT); }
            if (sel.size() == 1 && selectedSet() != null) { l.add("Browse as plain folder"); id.add(M_PLAIN); }
            l.add("Clear selection"); id.add(M_CLEAR);
        } else {
            l.add("Select all"); id.add(M_SELECT_ALL);
            l.add(showAll ? "Show archives only" : "Show all files"); id.add(M_ALL_FILES);
            l.add(showHidden ? "Hide hidden files" : "Show hidden files"); id.add(M_HIDDEN);
            l.add("Refresh"); id.add(M_REFRESH);
            l.add("About"); id.add(M_ABOUT);
        }
        int[] ids = new int[id.size()];
        for (int i = 0; i < ids.length; i++) ids[i] = id.get(i);
        menuSheet("", l.toArray(new String[0]), ids);
    }

    private void onMenu(int id) {
        switch (id) {
            case M_ALL_FILES:
                showAll = !showAll;
                prefs.edit().putBoolean("show_all", showAll).apply();
                loadDir(cwd);
                break;
            case M_HIDDEN:
                showHidden = !showHidden;
                prefs.edit().putBoolean("show_hidden", showHidden).apply();
                loadDir(cwd);
                break;
            case M_REFRESH: loadDir(cwd); break;
            case M_ABOUT: aboutDialog(); break;
            case M_SELECT_ALL:
                for (Row r : rows) sel.add(r.key);
                redraw();
                break;
            case M_CLEAR: sel.clear(); redraw(); break;
            case M_EXTRACT: {
                Item it = selectedArchive();
                if (it != null) openItem(it, () -> extractDialog(new ArrayList<String>()));
                break;
            }
            case M_PLAIN: {
                File dir = arc != null ? arc.file : new File(cwd, selectedSet().name);
                arc = null;
                sel.clear();
                loadDir(dir);
                break;
            }
            case M_RENAME: renameSheet(); break;
            case M_TEST: runTest(); break;
            case M_INFO: infoDialog(); break;
            case M_DECRYPT: decryptInPlace(); break;
            case M_EXTRACT_ALL: extractDialog(new ArrayList<String>()); break;
            default: break;
        }
    }

    private void sortSheet() {
        String[] names = {"Name", "Size", "Date modified", "Type"};
        Sheet s = new Sheet(this, "Sort by");
        for (int i = 0; i < names.length; i++) {
            final int k = i;
            s.option(names[i], sortKey == i, () -> {
                sortKey = k;
                saveSort();
            });
        }
        s.space(6);
        s.option("Ascending", sortAsc, () -> {
            sortAsc = true;
            saveSort();
        });
        s.option("Descending", !sortAsc, () -> {
            sortAsc = false;
            saveSort();
        });
        s.show();
    }

    private void saveSort() {
        prefs.edit().putInt("sort_key", sortKey).putBoolean("sort_asc", sortAsc).apply();
        sortRows();
        redraw();
    }

    /** Folders always come first; then the chosen key; ties fall back to the name. */
    private void sortRows() {
        final int key = sortKey;
        final boolean asc = sortAsc;
        Collections.sort(rows, (a, b) -> {
            boolean fa = a.folder || a.dir, fb = b.folder || b.dir;
            if (fa != fb) return fa ? -1 : 1;
            int c;
            switch (key) {
                case S_SIZE: c = Long.compare(a.size, b.size); break;
                case S_DATE: c = Long.compare(a.time, b.time); break;
                case S_TYPE: c = fa ? 0 : a.ext.compareTo(b.ext); break;
                default: c = a.title.compareToIgnoreCase(b.title); break;
            }
            if (c == 0) return a.title.compareToIgnoreCase(b.title);
            return asc ? c : -c;
        });
    }

    // ------------------------------------------------------------------ Python and permissions

    private void startPython() {
        new Thread(() -> {
            try {
                if (!Python.isStarted()) Python.start(new AndroidPlatform(MainActivity.this));
                Python py = Python.getInstance();
                String helper = getApplicationInfo().nativeLibraryDir + "/libkiln_accel.so";
                String note = py.getModule("kiln_boot").callAttr("init", helper).toString();
                app = py.getModule("kiln_app");
                ui.post(() -> {
                    if (!note.isEmpty()) toast("Slow mode: " + note);
                    refreshPermission();
                    renderBars();
                });
            } catch (Throwable t) {
                ui.post(() -> empty.setText("Kiln couldn't start: " + t));
            }
        }).start();
    }

    private boolean hasStorage() {
        return Build.VERSION.SDK_INT < 30 || Environment.isExternalStorageManager();
    }

    private void askStorage() {
        startActivity(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:" + getPackageName())));
    }

    private void refreshPermission() {
        boolean ok = hasStorage();
        permView.setVisibility(ok ? View.GONE : View.VISIBLE);
        if (ok && app != null && !loadedOnce) {
            loadedOnce = true;
            loadDir(cwd);
        }
        renderBars();
    }

    // ------------------------------------------------------------------ browsing

    private void loadDir(File dir) { loadDir(dir, false); }

    private void loadDir(File dir, boolean keepSelection) {
        cwd = dir;
        arc = null;
        if (!keepSelection) sel.clear();
        rows.clear();
        items.clear();
        empty.setText("Loading...");
        redraw();
        final String path = dir.getPath();
        final boolean all = showAll, hidden = showHidden;
        new Thread(() -> {
            JSONObject r;
            try {
                r = call("scan", path, all, hidden);
            } catch (Exception e) {
                r = errObj(String.valueOf(e.getMessage()));
            }
            final JSONObject res = r;
            ui.post(() -> {
                if (arc == null && cwd.getPath().equals(path)) applyScan(res);
            });
        }).start();
    }

    private void applyScan(JSONObject r) {
        rows.clear();
        items.clear();
        plainSet = r.optBoolean("is_set");
        if (!r.optBoolean("ok")) {
            empty.setText(r.optString("error", "Couldn't read this folder"));
            redraw();
            return;
        }
        JSONArray arr = r.optJSONArray("items");
        for (int i = 0; arr != null && i < arr.length(); i++) {
            JSONObject o = arr.optJSONObject(i);
            if (o == null) continue;
            Item it = new Item();
            it.name = o.optString("n");
            it.kind = o.optString("k");
            it.size = o.optLong("s");
            it.mtime = o.optLong("m");
            it.count = o.optInt("c");
            it.enc = o.optBoolean("e");
            items.add(it);
            Row row = new Row();
            row.key = it.name;
            row.title = it.name;
            row.item = it;
            row.time = it.mtime;
            row.ext = Fmt.ext(it.name);
            String when = Fmt.date(it.mtime);
            switch (it.kind) {
                case "dir":
                    row.folder = true;
                    row.size = it.count;
                    row.icon = R.drawable.ic_folder;
                    row.sub = when;
                    row.right = plural(it.count, "item", "items");
                    break;
                case "set":
                    row.folder = true;
                    row.icon = R.drawable.ic_set;
                    row.sub = "Encrypted set \u00b7 " + when;
                    row.right = "";
                    break;
                case "archive":
                    row.size = it.size;
                    row.icon = it.enc ? R.drawable.ic_lock : R.drawable.ic_archive;
                    row.sub = (it.enc ? "Encrypted archive \u00b7 " : "Archive \u00b7 ") + when;
                    row.right = Fmt.size(it.size);
                    break;
                case "old":
                    row.size = it.size;
                    row.icon = R.drawable.ic_archive;
                    row.sub = "Older Kiln format \u00b7 can't be opened here";
                    row.right = Fmt.size(it.size);
                    break;
                default:
                    row.size = it.size;
                    row.icon = R.drawable.ic_file;
                    row.sub = when;
                    row.right = Fmt.size(it.size);
                    break;
            }
            rows.add(row);
        }
        sel.retainAll(keysOf(rows));
        sortRows();
        empty.setText(showAll
                ? "This folder is empty."
                : "No Kiln archives here.\nOpen the menu to show all files.");
        redraw();
    }

    private static Collection<String> keysOf(List<Row> l) {
        List<String> k = new ArrayList<>();
        for (Row r : l) k.add(r.key);
        return k;
    }

    private void toggle(String key) {
        if (!sel.remove(key)) sel.add(key);
        redraw();
    }

    private void onRowClick(int pos) {
        if (pos < 0 || pos >= rows.size()) return;
        Row r = rows.get(pos);
        if (!sel.isEmpty()) {
            toggle(r.key);
            return;
        }
        if (arc != null) {
            if (r.dir) {
                arc.prefix = r.key;
                buildArcRows();
            } else {
                arcFileSheet(r);
            }
            return;
        }
        Item it = r.item;
        switch (it.kind) {
            case "dir": loadDir(new File(cwd, it.name)); break;
            case "archive":
            case "set": openItem(it, null); break;
            case "old":
                message("Older format", "This archive was made by an older Kiln (format 9 or earlier) "
                        + "that this app can't open.");
                break;
            default: fileSheet(new File(cwd, it.name), it); break;
        }
    }

    private List<File> selectedFiles() {
        List<File> out = new ArrayList<>();
        for (String n : sel) out.add(new File(cwd, n));
        return out;
    }

    private Item selectedArchive() {
        for (Item it : items) {
            if (sel.contains(it.name) && (it.kind.equals("archive") || it.kind.equals("set"))) return it;
        }
        return null;
    }

    private Item selectedSet() {
        for (Item it : items) {
            if (sel.contains(it.name) && it.kind.equals("set")) return it;
        }
        return null;
    }

    // ------------------------------------------------------------------ opening ordinary files

    private void fileSheet(final File f, Item it) {
        int kind = kindOf(f.getName());
        Sheet s = new Sheet(this, f.getName());
        s.text(Fmt.size(it.size) + " \u00b7 " + Fmt.date(it.mtime));
        boolean small = it.size <= TextActivity.MAX_BYTES;
        if (kind == 2 && it.size <= IMAGE_PREVIEW_MAX) {
            s.option("View in Kiln", false, () -> showImage(f, false));
        } else if (kind == 1 && small) {
            s.option("View in Kiln", false, () -> openText(f, false));
            s.option("Edit in Kiln", false, () -> openText(f, true));
        } else if (kind == 0 && small && it.size > 0) {
            s.option("View as text in Kiln", false, () -> openText(f, false));
            s.option("Edit as text in Kiln", false, () -> openText(f, true));
        }
        s.option("Open with...", false, () -> openExternal(f, false));
        s.option("Edit with...", false, () -> openExternal(f, true));
        s.button("Cancel", Ui.TEXT, null).show();
    }

    private void openText(File f, boolean edit) {
        needRefresh = edit;
        Intent i = new Intent(this, TextActivity.class);
        i.putExtra("path", f.getPath());
        i.putExtra("edit", edit);
        startActivity(i);
    }

    private static String mimeOf(File f) {
        String m = MimeTypeMap.getSingleton().getMimeTypeFromExtension(Fmt.ext(f.getName()));
        if (m != null) return m;
        return kindOf(f.getName()) == 1 ? "text/plain" : "*/*";
    }

    private void openExternal(File f, boolean edit) {
        try {
            Uri uri = FileProvider.getUriForFile(this, getPackageName() + ".files", f);
            Intent i = new Intent(edit ? Intent.ACTION_EDIT : Intent.ACTION_VIEW);
            i.setDataAndType(uri, mimeOf(f));
            i.setClipData(ClipData.newRawUri("", uri));
            i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                    | (edit ? Intent.FLAG_GRANT_WRITE_URI_PERMISSION : 0));
            needRefresh = edit;
            startActivity(Intent.createChooser(i, edit ? "Edit with" : "Open with"));
        } catch (Exception e) {
            toast("No app can open this file");
        }
    }

    private void showImage(final File f, final boolean cleanup) {
        BitmapFactory.Options o = new BitmapFactory.Options();
        o.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(f.getPath(), o);
        int sample = 1;
        while (o.outWidth / sample > 2048 || o.outHeight / sample > 2048) sample *= 2;
        o = new BitmapFactory.Options();
        o.inSampleSize = sample;
        Bitmap bmp = BitmapFactory.decodeFile(f.getPath(), o);
        if (cleanup && bmp == null) deleteTree(f.getParentFile());
        if (bmp == null) {
            message("Couldn't open", "This image can't be decoded.");
            return;
        }
        ImageView iv = new ImageView(this);
        iv.setImageBitmap(bmp);
        iv.setAdjustViewBounds(true);
        Sheet s = new Sheet(this, f.getName()).content(iv).button("Close", Ui.FILLED, null);
        if (cleanup) s.onClose(() -> deleteTree(f.getParentFile()));
        s.show();
    }

    // ------------------------------------------------------------------ opening an archive

    private void openItem(final Item it, final Runnable after) {
        final File f = new File(cwd, it.name);
        if (it.kind.equals("old")) {
            onRowClick(items.indexOf(it));
            return;
        }
        if (it.kind.equals("set") || it.enc) {
            askPassword(f.getName(), pw -> doOpen(f, pw, after));
        } else {
            doOpen(f, "", after);
        }
    }

    private void askPassword(String name, final OnText cb) {
        final EditText pw = Ui.field(this, "Password", true);
        final CheckBox show = Ui.check(this, "Show password", false);
        show.setOnCheckedChangeListener((b, on) -> Ui.setSecret(pw, on));
        new Sheet(this, "Unlock " + name).keyboard().content(pw).content(show)
                .button("Cancel", Ui.TEXT, null)
                .button("Unlock", Ui.FILLED, () -> {
                    String s = pw.getText().toString();
                    if (s.isEmpty()) {
                        pw.setError("Enter the password");
                        return false;
                    }
                    cb.got(s);
                    return true;
                }).show();
        pw.requestFocus();
    }

    private void doOpen(final File f, final String pw, final Runnable after) {
        final ProgressUi pu = new ProgressUi(pw.isEmpty() ? "Opening..." : "Unlocking...", null);
        new Thread(() -> {
            JSONObject r;
            try {
                r = call("open_archive", f.getPath(), pw);
            } catch (Exception e) {
                r = errObj(String.valueOf(e.getMessage()));
            }
            final JSONObject res = r;
            ui.post(() -> {
                pu.close();
                onOpened(f, pw, res, after);
            });
        }).start();
    }

    private void onOpened(final File f, final String pw, JSONObject r, final Runnable after) {
        if (!r.optBoolean("ok")) {
            if (r.optBoolean("need_password") || r.optBoolean("bad_password")) {
                if (r.optBoolean("bad_password")) toast("Wrong password");
                askPassword(f.getName(), p -> doOpen(f, p, after));
            } else {
                message("Couldn't open " + f.getName(), r.optString("error", "Unknown error"));
            }
            return;
        }
        Arc a = new Arc();
        a.file = f;
        a.password = pw;
        a.isSet = "set".equals(r.optString("kind"));
        a.encrypted = r.optBoolean("encrypted");
        a.packed = r.optLong("packed");
        JSONArray arr = r.optJSONArray("entries");
        for (int i = 0; arr != null && i < arr.length(); i++) {
            JSONObject o = arr.optJSONObject(i);
            if (o == null) continue;
            Ent e = new Ent();
            e.p = o.optString("p");
            e.s = o.optLong("s");
            e.m = o.optLong("m");
            a.ents.add(e);
        }
        arc = a;
        plainSet = false;
        sel.clear();
        buildArcRows();
        if (after != null) after.run();
    }

    private void closeArchive() {
        arc = null;
        sel.clear();
        loadDir(cwd);
    }

    private void buildArcRows() {
        rows.clear();
        String prefix = arc.prefix;
        Map<String, long[]> dirs = new HashMap<>();
        List<Ent> files = new ArrayList<>();
        for (Ent e : arc.ents) {
            if (!e.p.startsWith(prefix)) continue;
            String rest = e.p.substring(prefix.length());
            int i = rest.indexOf('/');
            if (i < 0) {
                files.add(e);
            } else {
                String d = rest.substring(0, i);
                long[] agg = dirs.get(d);
                if (agg == null) {
                    agg = new long[3];
                    dirs.put(d, agg);
                }
                agg[0]++;
                agg[1] += Math.max(e.s, 0);
                agg[2] = Math.max(agg[2], e.m);
            }
        }
        for (Map.Entry<String, long[]> en : dirs.entrySet()) {
            long[] agg = en.getValue();
            Row r = new Row();
            r.key = prefix + en.getKey() + "/";
            r.title = en.getKey();
            r.dir = true;
            r.icon = R.drawable.ic_folder;
            r.size = agg[1];
            r.time = agg[2];
            r.sub = plural(agg[0], "file", "files");
            r.right = Fmt.size(agg[1]);
            rows.add(r);
        }
        for (Ent e : files) {
            Row r = new Row();
            r.key = e.p;
            r.title = e.p.substring(prefix.length());
            r.ent = e;
            r.icon = R.drawable.ic_file;
            r.size = Math.max(e.s, 0);
            r.time = e.m;
            r.ext = Fmt.ext(r.title);
            r.sub = e.m > 0 ? Fmt.date(e.m) : "";
            r.right = Fmt.size(e.s);
            rows.add(r);
        }
        sortRows();
        empty.setText("This folder is empty.");
        redraw();
    }

    // ------------------------------------------------------------------ progress and jobs

    /** Modal progress sheet. cancelP == null hides the Cancel button. */
    private final class ProgressUi {
        final Sheet sheet;
        final ProgressBar bar;
        final TextView line;

        ProgressUi(String heading, final Progress cancelP) {
            line = Ui.label(MainActivity.this, "", 13, false, true);
            line.setSingleLine(true);
            line.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            bar = new ProgressBar(MainActivity.this, null, android.R.attr.progressBarStyleHorizontal);
            bar.setIndeterminate(true);
            bar.setMax(1000);
            ColorStateList acc = ColorStateList.valueOf(Ui.accent(MainActivity.this));
            bar.setProgressTintList(acc);
            bar.setIndeterminateTintList(acc);
            LinearLayout box = new LinearLayout(MainActivity.this);
            box.setOrientation(LinearLayout.VERTICAL);
            box.addView(line);
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            bp.topMargin = dp(12);
            box.addView(bar, bp);
            final Sheet sh = new Sheet(MainActivity.this, heading).content(box).cancelable(false);
            if (cancelP != null) {
                sh.button("Cancel", Ui.OUTLINE, () -> {
                    cancelP.cancel();
                    line.setText("Cancelling...");
                    sh.lastButton().setEnabled(false);
                    return false;
                });
            }
            sheet = sh;
            sh.show();
        }

        void update(double frac, String file) {
            if (!file.isEmpty()) line.setText(file);
            if (frac < 0) {
                bar.setIndeterminate(true);
            } else {
                bar.setIndeterminate(false);
                bar.setProgress((int) Math.round(frac * 1000));
            }
        }

        void close() { sheet.dismiss(); }
    }

    void onProgress(String json) {
        try {
            JSONObject o = new JSONObject(json);
            final double frac = o.optDouble("frac", -1);
            final String file = o.optString("file", "");
            ui.post(() -> {
                if (currentProgress != null) currentProgress.update(frac, file);
            });
            if (svcOn) {
                OpService.progress(this, file.isEmpty() ? "Working..." : file,
                        frac >= 0 ? (int) Math.round(frac * 100) : -1);
            }
        } catch (JSONException ignored) {
        }
    }

    private void runOp(final String heading, boolean cancellable, final Job job, final Done done) {
        if (Build.VERSION.SDK_INT >= 33 && !askedNotif
                && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            askedNotif = true;
            requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, 1);
        }
        final Progress p = new Progress(this);
        final ProgressUi pu = new ProgressUi(heading, cancellable ? p : null);
        currentProgress = pu;
        final boolean[] finished = {false};
        // only protect jobs that last: starting the service for a 50 ms job isn't worth it
        final Runnable startSvc = () -> {
            if (!finished[0]) {
                OpService.begin(MainActivity.this, heading);
                svcOn = true;
            }
        };
        ui.postDelayed(startSvc, 1500);
        new Thread(() -> {
            JSONObject r;
            try {
                r = job.run(p);
            } catch (Throwable t) {
                r = errObj(String.valueOf(t.getMessage()));
            }
            final JSONObject res = r;
            ui.post(() -> {
                finished[0] = true;
                ui.removeCallbacks(startSvc);
                if (svcOn) {
                    svcOn = false;
                    OpService.end(MainActivity.this);
                }
                pu.close();
                currentProgress = null;
                done.done(res);
            });
        }).start();
    }

    // ------------------------------------------------------------------ copy, move, delete

    private void setClip(boolean move) {
        clip.clear();
        clip.addAll(selectedFiles());
        clipMove = move;
        sel.clear();
        redraw();
        toast((move ? "Moving " : "Copying ") + plural(clip.size(), "item", "items")
                + ". Open a folder and tap \"" + (move ? "Move here" : "Paste here") + "\".");
    }

    private void paste() {
        final List<File> srcs = new ArrayList<>(clip);
        final boolean mv = clipMove;
        final File dest = cwd;
        final String json = pathsJson(srcs);
        bg(p -> call("fs_plan", json, dest.getPath(), mv), r -> {
            if (!r.optBoolean("ok")) {
                message("Can't " + (mv ? "move" : "copy"), r.optString("error", r.optString("message")));
                return;
            }
            JSONArray c = r.optJSONArray("conflicts");
            if (c == null || c.length() == 0) {
                runTransfer(json, dest, mv, "replace");
                return;
            }
            StringBuilder names = new StringBuilder();
            for (int i = 0; i < c.length() && i < 5; i++) names.append("\u2022 ").append(c.optString(i)).append('\n');
            if (c.length() > 5) names.append("and ").append(c.length() - 5).append(" more\n");
            new Sheet(this, plural(c.length(), "item already exists", "items already exist"))
                    .text(names.toString().trim())
                    .option("Replace", false, () -> runTransfer(json, dest, mv, "replace"))
                    .option("Skip existing", false, () -> runTransfer(json, dest, mv, "skip"))
                    .option("Keep both", false, () -> runTransfer(json, dest, mv, "keep"))
                    .button("Cancel", Ui.TEXT, null).show();
        });
    }

    private void runTransfer(final String json, final File dest, final boolean mv, final String policy) {
        runOp(mv ? "Moving..." : "Copying...", true,
                p -> call("fs_transfer", json, dest.getPath(), mv, policy, p), r -> {
                    clip.clear();
                    if (r.optBoolean("cancelled")) toast("Cancelled");
                    else if (r.optBoolean("ok")) toast(r.optString("message"));
                    else message("Not everything was " + (mv ? "moved" : "copied"), r.optString("message"));
                    loadDir(cwd);
                });
    }

    private void deleteSelected() {
        final List<File> victims = selectedFiles();
        if (victims.isEmpty()) return;
        String what = victims.size() == 1 ? "\"" + victims.get(0).getName() + "\""
                : plural(victims.size(), "item", "items");
        confirm("Delete " + what + "?", "This can't be undone.", "Delete", Ui.DANGER, () ->
                runOp("Deleting...", true, p -> call("fs_delete", pathsJson(victims), p), r -> {
                    if (r.optBoolean("cancelled")) toast("Cancelled");
                    else if (!r.optBoolean("ok")) message("Couldn't delete everything", r.optString("message"));
                    loadDir(cwd);
                }));
    }

    // ------------------------------------------------------------------ new, rename

    private void newSheet() {
        new Sheet(this, "New")
                .option("Folder", false, () -> nameSheet("New folder", "", "Create", n -> {
                    File nf = new File(cwd, n);
                    if (nf.exists()) return "That name is taken";
                    if (!nf.mkdirs()) return "Couldn't create it";
                    loadDir(cwd);
                    return null;
                }))
                .option("Text file", false, () -> nameSheet("New text file", "untitled.txt", "Create", n -> {
                    File nf = new File(cwd, n);
                    if (nf.exists()) return "That name is taken";
                    try {
                        if (!nf.createNewFile()) return "Couldn't create it";
                    } catch (java.io.IOException e) {
                        return String.valueOf(e.getMessage());
                    }
                    loadDir(cwd);
                    return null;
                }))
                .button("Cancel", Ui.TEXT, null).show();
    }

    private void nameSheet(String heading, String initial, String ok, final OnName cb) {
        final EditText name = Ui.field(this, "Name", false);
        name.setText(initial);
        new Sheet(this, heading).keyboard().content(name)
                .button("Cancel", Ui.TEXT, null)
                .button(ok, Ui.FILLED, () -> {
                    String n = name.getText().toString().trim();
                    if (n.isEmpty() || n.contains("/")) {
                        name.setError("Enter a name (no slashes)");
                        return false;
                    }
                    String err = cb.got(n);
                    if (err != null) {
                        name.setError(err);
                        return false;
                    }
                    return true;
                }).show();
        name.requestFocus();
        int dot = initial.lastIndexOf('.');
        name.setSelection(0, dot > 0 ? dot : initial.length());
    }

    private void renameSheet() {
        if (sel.size() != 1) return;
        final String old = sel.iterator().next();
        nameSheet("Rename", old, "Rename", n -> {
            File target = new File(cwd, n);
            if (target.exists()) return "That name is taken";
            if (!new File(cwd, old).renameTo(target)) return "Couldn't rename";
            loadDir(cwd);
            return null;
        });
    }

    // ------------------------------------------------------------------ archive actions

    private int countFor(Collection<String> keys) {
        if (keys.isEmpty()) return arc.ents.size();
        int n = 0;
        for (Ent e : arc.ents) {
            for (String k : keys) {
                if (k.endsWith("/") ? e.p.startsWith(k) : e.p.equals(k)) {
                    n++;
                    break;
                }
            }
        }
        return n;
    }

    private String membersJson(List<String> keys) throws JSONException {
        JSONArray a = new JSONArray();
        for (String k : keys) {
            JSONObject o = new JSONObject();
            boolean dir = k.endsWith("/");
            o.put("p", dir ? k.substring(0, k.length() - 1) : k);
            o.put("dir", dir);
            a.put(o);
        }
        return a.toString();
    }

    private void extractSelected() { extractDialog(new ArrayList<>(sel)); }

    private void arcFileSheet(final Row r) {
        final String name = r.title;
        int kind = kindOf(name);
        Sheet s = new Sheet(this, name);
        s.text(Fmt.size(r.ent.s) + (r.ent.m > 0 ? " \u00b7 " + Fmt.date(r.ent.m) : ""));
        boolean text = (kind == 1 || kind == 0) && r.ent.s <= TextActivity.MAX_BYTES && r.ent.s > 0;
        boolean image = kind == 2 && r.ent.s <= IMAGE_PREVIEW_MAX;
        if (text || image) s.option(kind == 0 ? "View as text in Kiln" : "View in Kiln", false,
                () -> previewFile(r.ent.p, 0));
        s.option("Open with...", false, () -> previewFile(r.ent.p, 1));
        s.option("Extract...", false, () -> {
            List<String> k = new ArrayList<>();
            k.add(r.key);
            extractDialog(k);
        });
        s.button("Cancel", Ui.TEXT, null).show();
    }

    /** Extracts one file to the cache, then shows it (mode 0) or hands it to another app (1). */
    private void previewFile(final String member, final int mode) {
        final Arc a = arc;
        final File cache = new File(getCacheDir(), "preview");
        cache.mkdirs();
        runOp("Preparing...", false,
                p -> call("preview", a.file.getPath(), a.password, member, cache.getPath()),
                r -> {
                    if (!r.optBoolean("ok")) {
                        message("Couldn't open", r.optString("message"));
                        return;
                    }
                    File f = new File(r.optString("file"));
                    if (mode == 1) {
                        openExternal(f, false);
                    } else if (kindOf(f.getName()) == 2) {
                        showImage(f, true);
                    } else {
                        openText(f, false);
                    }
                });
    }

    private void extractDialog(final List<String> keys) {
        final Arc a = arc;
        File defDest = new File(a.file.getParentFile(),
                a.isSet ? a.file.getName() + "_files" : Fmt.stripExt(a.file.getName()));
        final EditText where = Ui.field(this, "Folder", false);
        where.setText(defDest.getPath());
        final CheckBox replace = Ui.check(this, "Replace files that already exist", false);
        Button browse = Ui.button(this, "Browse", Ui.OUTLINE, v -> pickFolder(
                new File(where.getText().toString()), picked -> where.setText(picked.getPath())));
        new Sheet(this, "Extract").text(keys.isEmpty() ? "Extract all " + a.ents.size() + " files to:"
                : "Extract " + countFor(keys) + " file(s) to:")
                .content(where).space(8).content(browse).space(4).content(replace)
                .button("Cancel", Ui.TEXT, null)
                .button("Extract", Ui.FILLED, () -> {
                    String dest = where.getText().toString().trim();
                    if (dest.isEmpty()) {
                        where.setError("Choose a folder");
                        return false;
                    }
                    runExtract(keys, dest, replace.isChecked());
                    return true;
                }).show();
    }

    private void runExtract(final List<String> keys, final String dest, final boolean overwrite) {
        final Arc a = arc;
        final int total = countFor(keys);
        runOp("Extracting...", true,
                p -> call("extract", a.file.getPath(), a.password, membersJson(keys), dest,
                        overwrite, total, p),
                r -> {
                    String msg = r.optString("message");
                    if (r.optBoolean("ok")) {
                        sel.clear();
                        redraw();
                        new Sheet(this, "Done").text(msg)
                                .button("OK", Ui.TEXT, null)
                                .button("Open folder", Ui.FILLED, () -> {
                                    arc = null;
                                    loadDir(new File(dest));
                                    return true;
                                }).show();
                    } else if (r.optBoolean("cancelled")) {
                        toast("Cancelled");
                    } else if (!overwrite && msg.contains("already exist")) {
                        confirm("Replace existing files?", msg, "Replace", Ui.FILLED,
                                () -> runExtract(keys, dest, true));
                    } else {
                        message("Couldn't extract", msg);
                    }
                });
    }

    private void runTest() {
        final Arc a = arc;
        runOp("Checking...", true, p -> call("test", a.file.getPath(), a.password, p), r -> {
            if (r.optBoolean("ok")) {
                message("Archive is intact", r.optString("message"));
            } else if (r.optBoolean("cancelled")) {
                toast("Cancelled");
            } else {
                message("Problem found", r.optString("message"));
            }
        });
    }

    private void decryptInPlace() {
        final Arc a = arc;
        confirm("Decrypt in place?", "\"" + a.file.getName() + "\" becomes an ordinary folder: the "
                        + "encryption and the random file names are removed.",
                "Decrypt", Ui.FILLED, () -> runOp("Decrypting...", true,
                        p -> call("decrypt_in_place", a.file.getPath(), a.password, a.ents.size(), p),
                        r -> {
                            if (r.optBoolean("ok")) {
                                toast("Decrypted");
                                closeArchive();
                            } else if (r.optBoolean("cancelled")) {
                                toast("Cancelled");
                            } else {
                                message("Couldn't decrypt", r.optString("message"));
                            }
                        }));
    }

    private void infoDialog() {
        long raw = 0;
        for (Ent e : arc.ents) raw += Math.max(e.s, 0);
        StringBuilder sb = new StringBuilder();
        sb.append(arc.isSet ? "Encrypted set (one archive per file)\n" : "Archive\n");
        sb.append("Encrypted: ").append(arc.encrypted ? "yes (AES-256-GCM)" : "no").append('\n');
        sb.append("Files: ").append(arc.ents.size()).append('\n');
        sb.append("Original size: ").append(Fmt.size(raw)).append('\n');
        sb.append("Stored size: ").append(Fmt.size(arc.packed));
        if (raw > 0) sb.append(" (").append(Math.round(100.0 * arc.packed / raw)).append("%)");
        sb.append('\n').append("Location: ").append(arc.file.getPath());
        message(arc.file.getName(), sb.toString());
    }

    // ------------------------------------------------------------------ pack

    private void packDialog(final List<File> sources) {
        if (sources.isEmpty()) return;
        String base = sources.size() == 1 ? Fmt.stripExt(sources.get(0).getName())
                : (cwd.equals(root) ? "archive" : cwd.getName());
        final EditText name = Ui.field(this, "Name", false);
        name.setText(base + ".kln");
        final EditText where = Ui.field(this, "Save in", false);
        where.setText(cwd.getPath());
        final CheckBox enc = Ui.check(this, "Encrypt with a password", true);
        final EditText p1 = Ui.field(this, "Password (8+ characters)", true);
        final EditText p2 = Ui.field(this, "Repeat password", true);
        final CheckBox show = Ui.check(this, "Show passwords", false);
        final CheckBox store = Ui.check(this, "Don't compress (faster for photos, music, video)", false);
        final CheckBox each = Ui.check(this, "Separate archive for each file", false);
        final TextView note = Ui.label(this, "", 12, false, true);
        Button browse = Ui.button(this, "Browse", Ui.OUTLINE, v -> pickFolder(
                new File(where.getText().toString()), picked -> where.setText(picked.getPath())));
        show.setOnCheckedChangeListener((b, on) -> {
            Ui.setSecret(p1, on);
            Ui.setSecret(p2, on);
        });
        final Runnable sync = () -> {
            int vis = enc.isChecked() ? View.VISIBLE : View.GONE;
            p1.setVisibility(vis);
            p2.setVisibility(vis);
            show.setVisibility(vis);
            store.setVisibility(vis);
            String n = name.getText().toString();
            if (each.isChecked() && n.endsWith(".kln")) {
                name.setText(n.substring(0, n.length() - 4));
            } else if (!each.isChecked() && !n.endsWith(".kln") && !n.isEmpty()) {
                name.setText(n + ".kln");
            }
            note.setText(each.isChecked()
                    ? (enc.isChecked()
                    ? "Creates a folder. File names are replaced by random names; open the folder "
                    + "in Kiln to see the real ones."
                    : "Creates a folder with one archive per file.")
                    : "");
        };
        enc.setOnCheckedChangeListener((b, on) -> sync.run());
        each.setOnCheckedChangeListener((b, on) -> sync.run());
        sync.run();
        final Sheet s = new Sheet(this, sources.size() == 1
                ? "Pack \"" + sources.get(0).getName() + "\""
                : "Pack " + plural(sources.size(), "item", "items"));
        s.content(name).space(8).content(where).space(8).content(browse).space(8)
                .content(enc).content(p1).space(8).content(p2).content(show)
                .content(store).content(each).content(note)
                .button("Cancel", Ui.TEXT, null)
                .button("Create", Ui.FILLED, () -> {
                    String n = name.getText().toString().trim();
                    if (n.isEmpty() || n.contains("/")) {
                        name.setError("Enter a name (no slashes)");
                        return false;
                    }
                    File dir = new File(where.getText().toString().trim());
                    if (!dir.isDirectory()) {
                        where.setError("That folder doesn't exist");
                        return false;
                    }
                    final boolean encrypt = enc.isChecked();
                    final String pw = p1.getText().toString();
                    if (encrypt) {
                        if (pw.length() < 8) {
                            p1.setError("At least 8 characters");
                            return false;
                        }
                        if (!pw.equals(p2.getText().toString())) {
                            p2.setError("Passwords don't match");
                            return false;
                        }
                    }
                    final File dest = new File(dir, n);
                    final boolean separate = each.isChecked();
                    final boolean noCompress = store.isChecked();
                    if (dest.exists()) {
                        confirm("Replace \"" + n + "\"?", "It already exists.", "Replace", Ui.DANGER,
                                () -> runPack(sources, dest, encrypt, separate, noCompress, true, pw));
                    } else {
                        runPack(sources, dest, encrypt, separate, noCompress, false, pw);
                    }
                    return true;
                }).show();
    }

    private void runPack(final List<File> sources, final File dest, final boolean encrypt,
                         final boolean separate, final boolean noCompress,
                         final boolean overwrite, final String pw) {
        runOp("Packing...", true, p -> call("pack", pathsJson(sources), dest.getPath(), encrypt,
                separate, noCompress, overwrite, pw, p), r -> {
            if (r.optBoolean("ok")) {
                sel.clear();
                loadDir(cwd);
                message("Created " + dest.getName(), r.optString("message"));
            } else if (r.optBoolean("cancelled")) {
                toast("Cancelled");
            } else {
                message("Couldn't create the archive", r.optString("message"));
            }
        });
    }

    /** Folder chooser: tap a folder to enter it, then "Select this folder". */
    private void pickFolder(File start, final OnFile cb) {
        if (!hasStorage()) {
            askStorage();
            return;
        }
        File s = start;
        while (s != null && !s.isDirectory()) s = s.getParentFile();
        final File[] cur = {s != null ? s : root};
        final List<File> dirs = new ArrayList<>();
        final List<String> labels = new ArrayList<>();
        final ArrayAdapter<String> ad = new ArrayAdapter<>(this, android.R.layout.simple_list_item_1, labels);
        final ListView lv = new ListView(this);
        lv.setAdapter(ad);
        lv.setDivider(null);
        final TextView path = Ui.label(this, "", 13, false, true);
        path.setEllipsize(TextUtils.TruncateAt.START);
        path.setSingleLine(true);
        final Runnable[] fill = new Runnable[1];
        fill[0] = () -> {
            dirs.clear();
            labels.clear();
            dirs.add(null);
            labels.add("+  New folder...");
            File parent = cur[0].getParentFile();
            if (parent != null && !cur[0].equals(root.getParentFile())) {
                dirs.add(parent);
                labels.add(".. (up)");
            }
            File[] kids = cur[0].listFiles();
            List<File> sub = new ArrayList<>();
            if (kids != null) {
                for (File k : kids) {
                    if (k.isDirectory()) sub.add(k);
                }
            }
            Collections.sort(sub, (a, b) -> a.getName().compareToIgnoreCase(b.getName()));
            for (File k : sub) {
                dirs.add(k);
                labels.add(k.getName() + "/");
            }
            path.setText(cur[0].getPath());
            ad.notifyDataSetChanged();
        };
        lv.setOnItemClickListener((av, v, pos, id) -> {
            File target = dirs.get(pos);
            if (target == null) {
                nameSheet("New folder", "", "Create", n -> {
                    File nf = new File(cur[0], n);
                    if (nf.exists()) return "That name is taken";
                    if (!nf.mkdirs()) return "Couldn't create it";
                    cur[0] = nf;
                    fill[0].run();
                    return null;
                });
                return;
            }
            cur[0] = target;
            fill[0].run();
        });
        fill[0].run();
        new Sheet(this, "Choose a folder").content(path).fixed(lv, 300)
                .button("Cancel", Ui.TEXT, null)
                .button("Select this folder", Ui.FILLED, () -> {
                    cb.got(cur[0]);
                    return true;
                }).show();
    }

    private void aboutDialog() {
        String body = "Kiln " + getVersionName() + "\nOpen, create and unpack Kiln archives "
                + "(AES-256-GCM encryption), and manage ordinary files.";
        if (app != null) {
            try {
                JSONObject v = call("version");
                body += "\nEngine: kiln " + v.optString("kiln");
                String h = v.optString("helper");
                body += h.isEmpty() ? "\nFast mode: on" : "\nFast mode: off (" + h + ")";
            } catch (Exception ignored) {
            }
        }
        message("About Kiln", body);
    }

    private String getVersionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (Exception e) {
            return "";
        }
    }
}
