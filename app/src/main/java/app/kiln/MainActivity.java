package app.kiln;

import android.Manifest;
import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.database.Cursor;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.provider.OpenableColumns;
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
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.text.Editable;
import android.text.TextWatcher;
import android.widget.AbsListView;
import android.widget.PopupWindow;
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
import androidx.swiperefreshlayout.widget.SwipeRefreshLayout;

import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import com.chaquo.python.android.AndroidPlatform;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
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
    static final class Ent { String p, k = ""; long s, m; }
    static final class Row {
        String key, title, sub, right, ext = "", path;
        String where;                        // folder of a search hit, shown on its own line (null = none)
        int icon;
        boolean dir, folder, up;
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
        File home;                           // where extract defaults to (null = beside the file)
        String sid;                          // staged-edit session (null until the first edit)
        boolean dirty;                       // there are unsaved changes
        final List<Ent> ents = new ArrayList<>();
        final Map<String, int[]> pos = new HashMap<>();   // folder inside the archive -> {first row, offset}
    }

    private static final int S_NAME = 0, S_SIZE = 1, S_DATE = 2, S_TYPE = 3;
    private static final int M_ALL_FILES = 1, M_HIDDEN = 2, M_REFRESH = 3, M_ABOUT = 4,
            M_SELECT_ALL = 5, M_EXTRACT = 6, M_RENAME = 7, M_CLEAR = 9, M_TEST = 10,
            M_INFO = 11, M_DECRYPT = 12, M_EXTRACT_ALL = 13, M_PLAIN = 14, M_SETTINGS = 15,
            M_OPTIONS = 16, M_SAVE = 17, M_DISCARD = 18, M_PROPS = 19, M_ICON = 20;
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
    private volatile PyObject edit;          // kiln_edit module (staged edits inside archives)
    private final List<String> arcClip = new ArrayList<>();   // items held for paste inside an archive
    private boolean arcClipMove;
    private Arc pickArc;                     // archive waiting while the file browser picks files to add
    private File pickReturn;
    private String pickDest = "";
    private File editFile;                   // a copy of an archive file handed to an editor
    private String editMember;
    private long editStamp;
    private boolean editExternal;
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
    private boolean upRowOn = true;
    private boolean previewsOn = true;
    private Intent pendingIntent;            // a .kln opened from another app, waiting for Python
    private File cwd = Environment.getExternalStorageDirectory();
    private Arc arc;                         // non-null while an archive is open
    private ProgressUi currentProgress;

    private ImageButton left, sortBtn, more;
    private TextView title, sub, empty, banner;
    private ListView list;
    private View permView;
    private Button fab;
    private LinearLayout bottomBar;
    private SwipeRefreshLayout swipe;
    private FrameLayout body;
    private boolean barShown, fabShown;
    private RowAdapter adapter;

    // search
    private ImageButton searchBtn;
    private LinearLayout searchRow, titleBox;
    private EditText searchField;
    private Button deepChip;
    private boolean searching, deepOn, forceAnim, fabWanted, fabScrollHidden;
    private String query = "";
    private int searchGen, lastFirst, lastTop, imePad;
    private Runnable pendingSearch;
    private boolean deepRunning;
    private final List<Row> srcRows = new ArrayList<>();      // the folder (or archive level) itself
    private final List<Item> srcItems = new ArrayList<>();
    private PopupWindow marks;

    // ------------------------------------------------------------------ lifecycle

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = getSharedPreferences("kiln", MODE_PRIVATE);
        sortKey = prefs.getInt("sort_key", S_NAME);
        sortAsc = prefs.getBoolean("sort_asc", true);
        showAll = prefs.getBoolean("show_all", true);
        showHidden = prefs.getBoolean("show_hidden", false);
        deleteTree(new File(getCacheDir(), "incoming"));      // scratch copies; the preview cache is kept
        deleteTree(new File(getCacheDir(), "edit"));          // (and trimmed once Python is up)
        upRowOn = Defaults.upRow(this);
        previewsOn = Defaults.previews(this);
        installCrashLog();
        Priv.init(this);
        new Thread(Priv::clearStage).start();               // staged copies from an earlier run
        Priv.setListener(() -> ui.post(() -> {
            if (marks != null && marks.isShowing()) marks.dismiss();
        }));
        buildUi();
        startPython();
        ui.post(this::showLastCrash);
        if (state == null && Intent.ACTION_VIEW.equals(getIntent().getAction())) {
            pendingIntent = getIntent();
        }
    }

    /** If Kiln ever stops with an error, the details are kept and shown on the next start. */
    private void installCrashLog() {
        final Thread.UncaughtExceptionHandler prev = Thread.getDefaultUncaughtExceptionHandler();
        final File f = new File(getFilesDir(), "last-crash.txt");
        Thread.setDefaultUncaughtExceptionHandler((t, e) -> {
            try (java.io.PrintWriter w = new java.io.PrintWriter(f)) {
                w.println("Kiln " + getVersionName() + ", thread " + t.getName());
                e.printStackTrace(w);
            } catch (Throwable ignored) {
            }
            if (prev != null) prev.uncaughtException(t, e);
        });
    }

    private void showLastCrash() {
        File f = new File(getFilesDir(), "last-crash.txt");
        if (!f.isFile()) return;
        String text;
        try (java.io.FileInputStream in = new java.io.FileInputStream(f)) {
            byte[] b = new byte[(int) Math.min(f.length(), 2500)];
            int n = in.read(b);
            text = new String(b, 0, Math.max(n, 0), java.nio.charset.StandardCharsets.UTF_8);
        } catch (Exception e) {
            text = "";
        }
        f.delete();
        if (!text.isEmpty()) message("Kiln stopped last time", text);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        if (Intent.ACTION_VIEW.equals(intent.getAction())) {
            if (app == null) pendingIntent = intent;
            else openIncoming(intent);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        boolean up = Defaults.upRow(this), pv = Defaults.previews(this);
        boolean changed = up != upRowOn || pv != previewsOn;
        upRowOn = up;
        previewsOn = pv;
        if (changed && adapter != null) adapter.notifyDataSetChanged();
        refreshPermission();
        checkEditedFile();
        if (Priv.hasWatched()) {                               // a protected file was edited in another app
            new Thread(() -> {
                final List<String> sent = Priv.syncWatched();
                if (!sent.isEmpty()) ui.post(() -> {
                    needRefresh = true;
                    toast("Saved " + sent.get(0) + (sent.size() > 1 ? " and " + (sent.size() - 1) + " more" : ""));
                });
            }).start();
        }
        volAt = -10000;                                       // re-check which drives are plugged in
        if (arc == null && loadedOnce && hasStorage() && !cwd.isDirectory()) {
            File r = storageRootOf(cwd);                      // the drive was unplugged: go home
            loadDir(r.isDirectory() ? r : root);
            toast("That storage isn't available any more");
        } else if (needRefresh && arc == null && loadedOnce) {
            needRefresh = false;
            loadDir(cwd, true);
        }
    }

    @Override
    public void onBackPressed() {
        if (!sel.isEmpty()) {
            sel.clear();
            redraw();
        } else if (searching) {
            closeSearch();
        } else if (arc != null) {
            if (arc.prefix.isEmpty()) {
                guardLeave(this::closeArchive);
            } else {
                String p = arc.prefix.substring(0, arc.prefix.length() - 1);
                int i = p.lastIndexOf('/');
                arcBackTo(i < 0 ? "" : p.substring(0, i + 1));
            }
        } else if (!isStorageRoot(cwd) && cwd.getParentFile() != null) {
            loadDir(cwd.getParentFile());               // a flash drive's top is a home of its own
        } else if (pickArc != null) {
            endPick(false);
        } else {
            long now = android.os.SystemClock.elapsedRealtime();
            if (now - lastBackAtHome < 2000) {
                super.onBackPressed();
            } else {
                lastBackAtHome = now;
                toast("Tap Back again to exit");
            }
        }
    }

    private long lastBackAtHome;                      // when Back was last pressed in the home folder

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

    /**
     * What a real file on disk is: 1 text, 2 image, 0 "maybe text" (a name Kiln doesn't know whose
     * content reads as text, or an empty file), -1 binary. A name Kiln knows is trusted; anything
     * else (no extension, or an unfamiliar one) is looked at, so Kiln only offers its text viewer
     * for files that are text and can still show a picture that has lost its extension.
     */
    private static int kindOf(File f) {
        int k = kindOf(f.getName());
        if (k != 0) return k;
        return f.isFile() ? Sniff.kind(f) : 0;
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
        titles.setPadding(dp(8), dp(3), dp(8), dp(3));
        titles.setBackground(Ui.pressable(this, 0, 0, 12));
        titles.setOnClickListener(v -> showBookmarks());
        titleBox = titles;
        title.setCompoundDrawablePadding(dp(2));
        title.setCompoundDrawableTintList(ColorStateList.valueOf(Ui.text2(this)));
        bar.addView(titles, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        searchBtn = Ui.iconButton(this, R.drawable.ic_search, v -> {
            if (searching) closeSearch();
            else openSearch();
        });
        bar.addView(searchBtn, new LinearLayout.LayoutParams(dp(48), dp(48)));
        sortBtn = Ui.iconButton(this, R.drawable.ic_sort, v -> sortSheet());
        bar.addView(sortBtn, new LinearLayout.LayoutParams(dp(48), dp(48)));
        more = Ui.iconButton(this, R.drawable.ic_more, v -> showMenu());
        bar.addView(more, new LinearLayout.LayoutParams(dp(48), dp(48)));
        page.addView(bar);

        searchRow = new LinearLayout(this);
        searchRow.setOrientation(LinearLayout.HORIZONTAL);
        searchRow.setGravity(Gravity.CENTER_VERTICAL);
        searchRow.setPadding(dp(12), 0, dp(12), dp(8));
        searchRow.setVisibility(View.GONE);
        searchField = Ui.field(this, "Search", false);
        searchField.setImeOptions(EditorInfo.IME_ACTION_SEARCH | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        searchField.setCompoundDrawablesRelativeWithIntrinsicBounds(R.drawable.ic_search, 0, 0, 0);
        searchField.setCompoundDrawablePadding(dp(10));
        searchField.setCompoundDrawableTintList(ColorStateList.valueOf(Ui.text2(this)));
        searchField.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence c, int a, int b, int d) {}
            @Override public void onTextChanged(CharSequence c, int a, int b, int d) {}
            @Override public void afterTextChanged(Editable e) { onQueryChanged(e.toString()); }
        });
        searchField.setOnEditorActionListener((v, a, e) -> {
            hideKeyboard();
            return true;
        });
        searchRow.addView(searchField, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        deepChip = Ui.button(this, "Subfolders", Ui.TONAL, v -> {
            deepOn = !deepOn;
            Defaults.setDeepSearch(this, deepOn);
            renderSearchRow();
            onQueryChanged(searchField.getText().toString());
        });
        deepChip.setMinimumHeight(dp(44));
        LinearLayout.LayoutParams dc = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT);
        dc.leftMargin = dp(8);
        searchRow.addView(deepChip, dc);
        page.addView(searchRow);

        View line = new View(this);
        line.setBackgroundColor(Ui.line(this));
        page.addView(line, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(1)));

        banner = Ui.label(this, "", 13, false, false);
        banner.setPadding(dp(16), dp(10), dp(16), dp(10));
        banner.setBackgroundColor(getColor(R.color.k_warn_bg));
        banner.setVisibility(View.GONE);
        page.addView(banner);

        body = new FrameLayout(this);
        list = new ListView(this);
        list.setDivider(null);
        list.setSelector(android.R.color.transparent);
        list.setClipToPadding(false);
        list.setPadding(0, 0, 0, dp(10));                 // grows only while a bar or the keyboard is up
        adapter = new RowAdapter();
        list.setAdapter(adapter);
        list.setOnScrollListener(new AbsListView.OnScrollListener() {
            @Override
            public void onScrollStateChanged(AbsListView v, int state) {
                if (state == SCROLL_STATE_TOUCH_SCROLL && searching) hideKeyboard();
            }

            @Override
            public void onScroll(AbsListView v, int first, int visible, int total) {
                View c = v.getChildAt(0);
                if (c == null) return;
                int top = c.getTop();
                if (first == 0 && top >= 0) {
                    setFabScrollHidden(false);                 // back at the top: always show it
                } else if (first != lastFirst) {
                    setFabScrollHidden(first > lastFirst);
                } else if (Math.abs(top - lastTop) >= dp(6)) {
                    setFabScrollHidden(top < lastTop);         // content moving up = scrolling down
                } else {
                    return;
                }
                lastFirst = first;
                lastTop = top;
            }
        });
        list.setOnItemClickListener((av, v, pos, id) -> onRowClick(pos));
        list.setOnItemLongClickListener((av, v, pos, id) -> {
            if (dirPending) return true;
            int i = pos - off();
            if (i >= 0 && i < rows.size()) toggle(rows.get(i).key);
            return true;
        });
        list.setBackgroundColor(Ui.bg(this));            // covers the info panel until you pull
        buildPullPanel();
        body.addView(pullPanel, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, dp(PULL_H), Gravity.TOP));
        swipe = new PullSwipe(this);
        swipe.addView(list, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        swipe.setColorSchemeColors(Ui.accent(this));
        swipe.setProgressBackgroundColorSchemeColor(Ui.surface(this));
        swipe.setOnRefreshListener(this::onPull);
        body.addView(swipe, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        // not the list's empty view: the list has to stay on screen so pulling down still works
        empty = Ui.label(this, "Starting...", 15, false, true);
        empty.setGravity(Gravity.CENTER_HORIZONTAL);
        empty.setPadding(dp(32), 0, dp(32), 0);
        body.addView(empty, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP));

        fab = Ui.button(this, "+  New", Ui.FILLED, v -> newSheet());
        fab.setElevation(dp(4));
        fab.setVisibility(View.GONE);
        FrameLayout.LayoutParams fp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, dp(48), Gravity.BOTTOM | Gravity.END);
        fp.setMargins(0, 0, dp(16), dp(16));
        body.addView(fab, fp);

        bottomBar = new LinearLayout(this);
        bottomBar.setOrientation(LinearLayout.VERTICAL);
        bottomBar.setBackground(Ui.shape(this, Ui.surface(this), Ui.line(this), 20));
        bottomBar.setElevation(dp(8));
        bottomBar.setPadding(dp(6), dp(6), dp(6), dp(6));
        bottomBar.setVisibility(View.GONE);
        FrameLayout.LayoutParams bp2 = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM);
        bp2.setMargins(dp(12), 0, dp(12), dp(12));
        body.addView(bottomBar, bp2);
        bottomBar.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> padList());

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

        setContentView(page);
        page.setOnApplyWindowInsetsListener((v, ins) -> {
            int ime = ins.getInsets(WindowInsets.Type.ime()).bottom;
            list.post(() -> {
                int pad = 0;
                if (ime > 0 && list.getHeight() > 0) {
                    int[] loc = new int[2];
                    list.getLocationInWindow(loc);
                    int kbTop = list.getRootView().getHeight() - ime;
                    pad = Math.max(0, loc[1] + list.getHeight() - kbTop);
                }
                if (pad != imePad) {
                    imePad = pad;
                    padList();
                }
            });
            return v.onApplyWindowInsets(ins);
        });
        renderHeader();
    }

    /** Bottom space under the list: a little normally, more while a bar or the keyboard covers it. */
    private void padList() {
        int bottom = (barShown ? bottomBar.getHeight() + dp(24) : dp(10)) + imePad;
        if (list.getPaddingBottom() != bottom) list.setPadding(0, 0, 0, bottom);
    }

    private void setFabScrollHidden(boolean hide) {
        if (fabScrollHidden == hide) return;
        fabScrollHidden = hide;
        showFab(fabWanted && !fabScrollHidden);
    }

    /** The ".." row shows when there's somewhere to go up to. */
    private boolean upShown() {
        if (upFrozen != null) return upFrozen;
        return upRowOn && (arc != null || !isStorageRoot(cwd)) && hasStorage()
                && !(searching && !query.isEmpty());
    }

    private int off() { return upShown() ? 1 : 0; }

    private Row rowAt(int adapterPos) {
        int i = adapterPos - off();
        return i >= 0 && i < rows.size() ? rows.get(i) : null;
    }

    /** One row: icon (or preview), name and date, size on the right. A selected row is tinted. */
    private final class RowView extends LinearLayout {
        final ImageView icon;
        final TextView name, detail, loc, size;
        private final Drawable plain, picked;
        private boolean wasSelected;

        RowView() {
            super(MainActivity.this);
            setOrientation(HORIZONTAL);
            setGravity(Gravity.CENTER_VERTICAL);
            setPadding(dp(16), dp(9), dp(16), dp(9));
            plain = Ui.ripple(MainActivity.this, 0, 0, 0, Ui.alpha(Ui.text(MainActivity.this), 0x22));
            picked = Ui.ripple(MainActivity.this, getColor(R.color.k_sel), 0, 0,
                    Ui.alpha(Ui.text(MainActivity.this), 0x22));
            icon = Thumbs.box(MainActivity.this);
            addView(icon, new LayoutParams(dp(40), dp(40)));
            LinearLayout col = new LinearLayout(MainActivity.this);
            col.setOrientation(VERTICAL);
            name = Ui.label(MainActivity.this, "", 16, false, false);
            name.setSingleLine(true);
            name.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            detail = Ui.label(MainActivity.this, "", 12, false, true);
            detail.setSingleLine(true);
            detail.setEllipsize(TextUtils.TruncateAt.END);
            loc = Ui.label(MainActivity.this, "", 12, false, true);
            loc.setSingleLine(true);
            loc.setEllipsize(TextUtils.TruncateAt.START);       // keep the deepest folder in view
            loc.setTextColor(Ui.accent(MainActivity.this));
            col.addView(name);
            col.addView(detail);
            col.addView(loc);
            LayoutParams cp = new LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            cp.leftMargin = dp(14);
            addView(col, cp);
            size = Ui.label(MainActivity.this, "", 13, false, true);
            size.setSingleLine(true);
            LayoutParams sp = new LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT);
            sp.leftMargin = dp(12);
            addView(size, sp);
        }

        void bind(Row r, boolean selected) {
            int accent = Ui.accent(getContext());
            if (selected) {
                icon.animate().cancel();
                Thumbs.showIcon(icon, R.drawable.ic_check, accent);
                icon.setTag(null);
            } else if (r.path != null && !r.up) {
                File f = new File(r.path);
                Thumbs.bind(icon, f, r.icon, r.folder || r.dir ? accent : Icons.tintFor(getContext(), r.title),
                        previewsOn);
            } else {
                icon.setTag(null);
                Thumbs.showIcon(icon, r.icon, r.folder || r.dir || r.up ? accent
                        : r.item != null && !r.item.kind.equals("file") ? Ui.text2(getContext())
                        : Icons.tintFor(getContext(), r.title));
            }
            if (selected != wasSelected) {                    // a little pop when selection flips
                icon.setScaleX(0.7f);
                icon.setScaleY(0.7f);
                icon.animate().scaleX(1f).scaleY(1f).setDuration(Defaults.anim(getContext(), 180)).start();
            }
            wasSelected = selected;
            name.setText(r.title);
            detail.setText(r.sub);
            detail.setVisibility(r.sub.isEmpty() ? View.GONE : View.VISIBLE);
            String w = r.where;                               // where a search hit lives
            if (w == null && r.item != null && arc == null && searching && !query.isEmpty()) {
                w = whereLabel("");                           // hit in the searched folder itself
            }
            loc.setText(w == null ? "" : w);
            loc.setVisibility(w == null || w.isEmpty() ? View.GONE : View.VISIBLE);
            size.setText(r.right);
            setBackground(selected ? picked : plain);
            jumpDrawablesToCurrentState();
        }
    }

    private final class RowAdapter extends BaseAdapter {
        @Override public int getCount() { return rows.size() + off(); }
        @Override public Object getItem(int i) { return rowAt(i); }
        @Override public long getItemId(int i) { return i; }

        @Override
        public View getView(int i, View convert, ViewGroup parent) {
            RowView v = convert instanceof RowView ? (RowView) convert : new RowView();
            Row r = rowAt(i);
            if (r == null) {
                v.bind(upRow(), false);
            } else {
                v.bind(r, sel.contains(r.key));
            }
            return v;
        }
    }

    /**
     * The rows slide in, once, staggered from the top. It starts on the first draw after the
     * list's new data is laid out and runs on the rows themselves, so no later layout pass
     * (selection, thumbnails, a rescan) can replay it. Call after the data has changed.
     */
    private void playRows() {
        if (!Defaults.animOn(this)) return;
        list.getViewTreeObserver().addOnPreDrawListener(new android.view.ViewTreeObserver.OnPreDrawListener() {
            @Override
            public boolean onPreDraw() {
                android.view.ViewTreeObserver o = list.getViewTreeObserver();
                if (o.isAlive()) o.removeOnPreDrawListener(this);
                int n = list.getChildCount();
                for (int k = 0; k < n; k++) {
                    View c = list.getChildAt(k);
                    c.animate().cancel();
                    c.setAlpha(0f);
                    c.setTranslationY(c.getHeight() * 0.14f);
                    c.animate().alpha(1f).translationY(0f)
                            .setStartDelay(Defaults.anim(MainActivity.this, 29L * Math.min(k, 24)))
                            .setDuration(Defaults.anim(MainActivity.this, 240))
                            .setInterpolator(new android.view.animation.DecelerateInterpolator(1.5f))
                            .start();
                }
                return true;
            }
        });
    }

    private Row upRow;

    private Row upRow() {
        if (upRow == null) {
            upRow = new Row();
            upRow.up = true;
            upRow.key = "..";
            upRow.title = "..";
            upRow.sub = "";
            upRow.right = "";
            upRow.icon = R.drawable.ic_up_dir;
        }
        return upRow;
    }

    // ------------------------------------------------------------------ header, bars, menus

    private void redraw() {
        adapter.notifyDataSetChanged();
        updateEmpty();
        renderHeader();
    }

    private void updateEmpty() {
        empty.setVisibility(rows.isEmpty() ? View.VISIBLE : View.GONE);
        FrameLayout.LayoutParams lp = (FrameLayout.LayoutParams) empty.getLayoutParams();
        int top = dp(upShown() ? 96 : 56);
        if (lp.topMargin != top) {
            lp.topMargin = top;
            empty.setLayoutParams(lp);
        }
    }

    // ------------------------------------------------------------------ pull-down info
    // Dragging the list down at the top reveals a strip behind it (type, free space, name, size and
    // a usage bar, like ZArchiver) while the usual pull-to-refresh still runs on release.

    private static final int PULL_H = 84;
    private LinearLayout pullPanel;
    private TextView pullType, pullFree, pullName, pullSize;
    private ProgressBar pullBar;
    private boolean pullShown;
    private java.util.concurrent.atomic.AtomicBoolean pullStop = new java.util.concurrent.atomic.AtomicBoolean(true);

    private void buildPullPanel() {
        pullPanel = new LinearLayout(this);
        pullPanel.setOrientation(LinearLayout.VERTICAL);
        pullPanel.setBackgroundColor(Ui.surface(this));
        pullPanel.setPadding(dp(16), dp(10), dp(16), 0);
        pullPanel.setVisibility(View.INVISIBLE);
        LinearLayout grid = new LinearLayout(this);
        LinearLayout l = new LinearLayout(this), r = new LinearLayout(this);
        l.setOrientation(LinearLayout.VERTICAL);
        r.setOrientation(LinearLayout.VERTICAL);
        pullType = Ui.label(this, "", 13, false, false);
        pullFree = Ui.label(this, "", 13, false, false);
        pullName = Ui.label(this, "", 13, false, false);
        pullSize = Ui.label(this, "", 13, false, false);
        for (TextView t : new TextView[]{pullType, pullFree, pullName, pullSize}) {
            t.setSingleLine(true);
            t.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        }
        l.addView(pullType);
        l.addView(pullFree);
        r.addView(pullName);
        r.addView(pullSize);
        grid.addView(l, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        rp.leftMargin = dp(12);
        grid.addView(r, rp);
        pullPanel.addView(grid);
        pullBar = Ui.meter(this, 0, true);
        LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(6));
        bp.topMargin = dp(10);
        pullPanel.addView(pullBar, bp);
    }

    /** Fills the strip for wherever you are: a folder, or the archive that is open. */
    private void fillPull() {
        pullStop.set(true);
        final java.util.concurrent.atomic.AtomicBoolean stop = new java.util.concurrent.atomic.AtomicBoolean();
        pullStop = stop;
        final File at = arc != null ? arc.file.getParentFile() : cwd;
        long[] sp = at != null ? spaceOf(at) : null;
        pullFree.setText(sp != null ? "Free: " + Fmt.size(sp[0]) + " / " + Fmt.size(sp[1]) : "");
        pullBar.setProgress(sp != null && sp[1] > 0
                ? (int) Math.max(8, Math.round(1000.0 * (sp[1] - sp[0]) / sp[1])) : 0);
        if (arc != null) {
            pullType.setText(arc.isSet ? "Type: Archive set" : "Type: Kiln archive");
            pullName.setText("Archive: " + arc.file.getName());
            pullSize.setText("Size: " + Fmt.size(arc.file.length()) + " \u00b7 "
                    + plural(arc.ents.size(), "file", "files"));
            return;
        }
        pullType.setText("Type: " + (isStorageRoot(cwd) ? "Storage" : "Folder"));
        pullName.setText((isStorageRoot(cwd) ? "Storage: " + storageName(cwd) : "Folder: " + cwd.getName()));
        pullSize.setText("Size: ...");
        new Thread(() -> {                                   // the folder's size climbs as it is counted
            final long[] tot = {0};
            if (Priv.wants(at.getPath())) {
                long v = Priv.du(at.getPath());
                if (v >= 0) tot[0] = v;
            } else {
                walkSize(at, stop, tot, 0);
            }
            if (!stop.get()) ui.post(() -> {
                if (!stop.get()) pullSize.setText("Size: " + Fmt.size(tot[0]));
            });
        }).start();
    }

    private void walkSize(File d, java.util.concurrent.atomic.AtomicBoolean stop, long[] tot, int depth) {
        File[] kids = d.listFiles();
        if (kids == null || depth > 40) return;
        for (File k : kids) {
            if (stop.get()) return;
            if (k.isDirectory()) walkSize(k, stop, tot, depth + 1);
            else tot[0] += k.length();
            long now = android.os.SystemClock.uptimeMillis();
            if (now - pullPost >= 120) {
                pullPost = now;
                final long v = tot[0];
                ui.post(() -> {
                    if (!stop.get()) pullSize.setText("Size: " + Fmt.size(v) + "...");
                });
            }
        }
    }

    private long pullPost;

    private void pullMove(float dy) {
        if (!sel.isEmpty() || !hasStorage()) return;
        float reveal = Math.max(0f, Math.min(dy * 0.6f, dp(PULL_H)));
        if (reveal > 0f && !pullShown) {
            pullShown = true;
            list.animate().cancel();
            empty.animate().cancel();
            pullPanel.animate().cancel();
            fillPull();
            pullPanel.setVisibility(View.VISIBLE);
        }
        if (pullShown) {
            list.setTranslationY(reveal);
            empty.setTranslationY(reveal);                   // the message belongs to the list: move together
            pullPanel.setAlpha(Math.min(1f, reveal / (dp(PULL_H) * 0.5f)));
        }
    }

    private void pullEnd() {
        if (!pullShown) return;
        pullShown = false;
        boolean full = list.getTranslationY() >= dp(PULL_H) * 0.9f;
        final long delay = full ? 250 : 0;
        final long dur = Defaults.anim(this, 220);
        empty.animate().translationY(0f).setStartDelay(delay).setDuration(dur)
                .withEndAction(() -> empty.animate().setStartDelay(0)).start();
        list.animate().translationY(0f).setStartDelay(delay)
                .setDuration(dur).withEndAction(() -> {
                    list.animate().setStartDelay(0);
                    pullPanel.setVisibility(View.INVISIBLE);
                    pullStop.set(true);
                }).start();
    }

    /** Pull-to-refresh that also reports how far the list is dragged, so the strip can be shown. */
    private final class PullSwipe extends SwipeRefreshLayout {
        private boolean atTop;
        private float baseY;

        PullSwipe(android.content.Context c) { super(c); }

        @Override
        public boolean dispatchTouchEvent(MotionEvent e) {
            switch (e.getActionMasked()) {
                case MotionEvent.ACTION_MOVE:
                    if (e.getPointerCount() == 1 && list != null) {
                        if (!list.canScrollVertically(-1)) {
                            if (!atTop) {
                                atTop = true;
                                baseY = e.getY();
                            }
                            pullMove(e.getY() - baseY);
                        } else {
                            atTop = false;
                        }
                    }
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    atTop = false;
                    pullEnd();
                    break;
                default:
                    break;
            }
            return super.dispatchTouchEvent(e);
        }
    }

    private void onPull() {
        if (app == null || !hasStorage()) {
            swipe.setRefreshing(false);
        } else if (arc != null) {
            buildArcRows();                                 // an open archive doesn't change
            swipe.setRefreshing(false);
        } else {
            forceAnim = true;                               // the rows slide in again
            loadDir(cwd, true);
        }
    }

    private void renderHeader() {
        boolean selecting = !sel.isEmpty();
        left.setVisibility(View.VISIBLE);
        if (selecting || arc != null) leftWasHome = false;
        sortBtn.setVisibility(selecting ? View.GONE : View.VISIBLE);
        searchBtn.setVisibility(selecting ? View.GONE : View.VISIBLE);
        searchBtn.setImageResource(searching ? R.drawable.ic_close : R.drawable.ic_search);
        boolean marksOk = !selecting && arc == null && hasStorage();
        titleBox.setClickable(marksOk);
        title.setCompoundDrawablesRelativeWithIntrinsicBounds(0, 0, marksOk ? R.drawable.ic_down : 0, 0);
        if (selecting) {
            setLeft(R.drawable.ic_close, true);
            title.setText(sel.size() + " selected");
            sub.setText("");
        } else if (arc != null) {
            setLeft(R.drawable.ic_up, true);
            title.setText(arc.file.getName() + (arc.dirty ? " *" : ""));
            sub.setText((arc.prefix.isEmpty()
                    ? plural(arc.ents.size(), "file", "files") + (arc.encrypted ? " \u00b7 encrypted" : "")
                    : "/" + arc.prefix) + (arc.dirty ? " \u00b7 unsaved changes" : ""));
        } else {
            boolean atRoot = isStorageRoot(cwd);
            if (atRoot) {
                setLeft(R.drawable.ic_kiln, false);          // home: the Kiln mark fills the corner
                if (!leftWasHome) glowMark();
            } else {
                setLeft(R.drawable.ic_up, true);
            }
            leftWasHome = atRoot;
            title.setText(pickArc != null ? "Add to " + pickArc.file.getName()
                    : atRoot ? (cwd.equals(root) ? "Kiln" : storageName(cwd)) : cwd.getName());
            sub.setText(cwd.getPath());
        }
        banner.setVisibility(plainSet && arc == null ? View.VISIBLE : View.GONE);
        banner.setText("Plain view of an encrypted set. Renaming or deleting files here can make "
                + "the set unreadable.");
        renderBars();
    }

    private boolean leftWasHome;

    private void setLeft(int res, boolean tinted) {
        left.setImageResource(res);
        left.setImageTintList(tinted ? ColorStateList.valueOf(Ui.text(this)) : null);
    }

    /** A short warm pulse on the mark when you arrive at home. */
    private void glowMark() {
        left.animate().cancel();
        if (!Defaults.animOn(this)) {
            left.setScaleX(1f);
            left.setScaleY(1f);
            return;
        }
        left.setScaleX(0.8f);
        left.setScaleY(0.8f);
        left.animate().scaleX(1f).scaleY(1f).setDuration(Defaults.anim(this, 420))
                .setInterpolator(new android.view.animation.OvershootInterpolator(2.2f)).start();
    }

    private void renderBars() {
        bottomBar.removeAllViews();
        boolean selecting = !sel.isEmpty();
        boolean show = true;
        int accent = Ui.accent(this), danger = getColor(R.color.k_danger);
        if (pickArc != null) {
            pickBar(selecting, accent);
        } else if (arc != null) {
            if (!arcClip.isEmpty()) {
                arcPasteBar(accent);
            } else {
                if (arc.dirty) bottomBar.addView(saveRow());
                if (selecting) {
                    tiles(tile(R.drawable.ic_extract, "Extract", accent, this::extractSelected),
                            tile(R.drawable.ic_copy, "Copy", Ui.text(this), () -> arcSetClip(false)),
                            tile(R.drawable.ic_move, "Move", Ui.text(this), () -> arcSetClip(true)),
                            tile(R.drawable.ic_delete, "Delete", danger, this::arcDelete));
                } else {
                    tiles(tile(R.drawable.ic_extract, "Extract all", accent,
                                    () -> extractDialog(new ArrayList<String>())),
                            tile(R.drawable.ic_add, "Add", accent, this::arcNewSheet),
                            tile(R.drawable.ic_check, "Check", Ui.text(this), this::runTest),
                            tile(R.drawable.ic_archive, "Info", Ui.text(this), this::infoDialog));
                }
            }
        } else if (selecting) {
            tiles(tile(R.drawable.ic_copy, "Copy", Ui.text(this), () -> setClip(false)),
                    tile(R.drawable.ic_move, "Move", Ui.text(this), () -> setClip(true)),
                    tile(R.drawable.ic_delete, "Delete", danger, this::deleteSelected),
                    tile(R.drawable.ic_archive, "Pack", accent, () -> packDialog(selectedFiles())));
        } else if (!clip.isEmpty()) {
            LinearLayout info = new LinearLayout(this);
            info.setOrientation(LinearLayout.VERTICAL);
            info.setPadding(dp(12), dp(8), dp(12), dp(2));
            info.addView(Ui.label(this, (clipMove ? "Moving " : "Copying ")
                    + plural(clip.size(), "item", "items"), 15, true, false));
            info.addView(Ui.label(this, "Into " + (isStorageRoot(cwd) ? storageName(cwd) : cwd.getName()),
                    12, false, true));
            bottomBar.addView(info);
            LinearLayout row = new LinearLayout(this);
            row.setGravity(Gravity.CENTER_VERTICAL);
            row.setPadding(dp(2), dp(4), dp(4), dp(2));
            View nf = tile(R.drawable.ic_new_folder, "New folder", accent, this::newFolderHere);
            row.addView(nf, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT));
            row.addView(new View(this), new LinearLayout.LayoutParams(0, 1, 1f));
            row.addView(Ui.button(this, "Cancel", Ui.TEXT, v -> {
                clip.clear();
                redraw();
            }));
            Button go = Ui.button(this, clipMove ? "Move here" : "Paste here", Ui.FILLED, v -> paste());
            LinearLayout.LayoutParams gp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            gp.leftMargin = dp(6);
            row.addView(go, gp);
            bottomBar.addView(row);
        } else {
            show = false;
        }
        showBar(show);
        fabWanted = arc == null && pickArc == null && arcClip.isEmpty() && !selecting && clip.isEmpty()
                && app != null && hasStorage() && !searching;
        showFab(fabWanted && !fabScrollHidden);
    }

    /** Icon over a small label; the whole tile lights up when touched. */
    private View tile(int icon, String label, int color, final Runnable r) {
        LinearLayout t = new LinearLayout(this);
        t.setOrientation(LinearLayout.VERTICAL);
        t.setGravity(Gravity.CENTER);
        t.setPadding(dp(10), dp(8), dp(10), dp(6));
        t.setBackground(Ui.pressable(this, 0, 0, 14));
        Ui.pressScale(t);
        ImageView iv = new ImageView(this);
        iv.setImageResource(icon);
        Ui.tint(iv, color);
        t.addView(iv, new LinearLayout.LayoutParams(dp(24), dp(24)));
        TextView tv = Ui.label(this, label, 11, false, false);
        tv.setTextColor(color);
        tv.setSingleLine(true);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.topMargin = dp(2);
        t.addView(tv, lp);
        t.setOnClickListener(v -> r.run());
        return t;
    }

    private void tiles(View... views) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        for (View v : views) {
            row.addView(v, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        }
        bottomBar.addView(row);
    }

    private void showBar(boolean on) {
        if (on && !barShown) {
            bottomBar.animate().cancel();
            bottomBar.setVisibility(View.VISIBLE);
            bottomBar.setAlpha(0f);
            bottomBar.setTranslationY(dp(70));
            bottomBar.animate().alpha(1f).translationY(0).setDuration(Defaults.anim(this, 240))
                    .setInterpolator(new android.view.animation.DecelerateInterpolator(1.8f)).start();
        } else if (!on && barShown) {
            bottomBar.animate().cancel();
            bottomBar.animate().alpha(0f).translationY(dp(70)).setDuration(Defaults.anim(this, 160))
                    .withEndAction(() -> {
                        if (!barShown) bottomBar.setVisibility(View.GONE);
                    }).start();
        }
        barShown = on;
        padList();
    }

    private void showFab(boolean on) {
        if (on && !fabShown) {
            fab.animate().cancel();
            fab.setVisibility(View.VISIBLE);
            fab.setAlpha(0f);
            fab.setScaleX(0.6f);
            fab.setScaleY(0.6f);
            fab.animate().alpha(1f).scaleX(1f).scaleY(1f).setDuration(Defaults.anim(this, 200))
                    .setInterpolator(new android.view.animation.OvershootInterpolator(1.4f)).start();
        } else if (!on && fabShown) {
            fab.animate().cancel();
            fab.animate().alpha(0f).scaleX(0.6f).scaleY(0.6f).setDuration(Defaults.anim(this, 120))
                    .withEndAction(() -> {
                        if (!fabShown) fab.setVisibility(View.GONE);
                    }).start();
        }
        fabShown = on;
    }

    private void onLeft() {
        if (!sel.isEmpty()) {
            sel.clear();
            redraw();
        } else if (arc == null && !searching && isStorageRoot(cwd)) {
            glowMark();                                      // at home the mark isn't "up": tap for About
            aboutDialog();
        } else {
            onBackPressed();
        }
    }

    /** A bottom-sheet menu: labels and the ids onMenu() understands. */
    private void menuSheet(String heading, String[] labels, final int[] ids) {
        Sheet s = new Sheet(this, heading);
        for (int i = 0; i < labels.length; i++) {
            final int id = ids[i];
            s.option(menuIcon(id), labels[i], false, () -> onMenu(id));
        }
        s.show();
    }

    private static int menuIcon(int id) {
        switch (id) {
            case M_SETTINGS: return R.drawable.ic_settings;
            case M_REFRESH: return R.drawable.ic_refresh;
            case M_SELECT_ALL: return R.drawable.ic_check;
            case M_CLEAR: return R.drawable.ic_close;
            case M_EXTRACT:
            case M_EXTRACT_ALL: return R.drawable.ic_extract;
            case M_RENAME: return R.drawable.ic_edit;
            case M_TEST: return R.drawable.ic_check;
            case M_PLAIN:
            case M_ICON: return R.drawable.ic_folder;
            case M_DECRYPT: return R.drawable.ic_lock;
            case M_OPTIONS: return R.drawable.ic_open;
            case M_INFO:
            case M_ABOUT: return R.drawable.ic_archive;
            default: return R.drawable.ic_file;
        }
    }

    private void showMenu() {
        List<String> l = new ArrayList<>();
        List<Integer> id = new ArrayList<>();
        boolean selecting = !sel.isEmpty();
        if (arc != null) {
            if (selecting) {
                if (sel.size() == 1) { l.add("Rename"); id.add(M_RENAME); }
                l.add("Properties"); id.add(M_PROPS);
                l.add("Select all"); id.add(M_SELECT_ALL);
                l.add("Clear selection"); id.add(M_CLEAR);
            } else {
                if (arc.dirty) { l.add("Save changes"); id.add(M_SAVE); l.add("Discard changes"); id.add(M_DISCARD); }
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
            l.add("Properties"); id.add(M_PROPS);
            if (sel.size() == 1 && !sel.iterator().next().contains("/")) { l.add("Rename"); id.add(M_RENAME); }
            if (sel.size() == 1 && selectedDir() != null) { l.add("Change icon..."); id.add(M_ICON); }
            if (sel.size() == 1 && selectedPlainFile() != null) { l.add("Open options..."); id.add(M_OPTIONS); }
            if (sel.size() == 1 && selectedArchive() != null) { l.add("Extract..."); id.add(M_EXTRACT); }
            if (sel.size() == 1 && selectedSet() != null) { l.add("Browse as plain folder"); id.add(M_PLAIN); }
            l.add("Clear selection"); id.add(M_CLEAR);
        } else {
            l.add("Select all"); id.add(M_SELECT_ALL);
            l.add(showAll ? "Show archives only" : "Show all files"); id.add(M_ALL_FILES);
            l.add(showHidden ? "Hide hidden files" : "Show hidden files"); id.add(M_HIDDEN);
            l.add("Refresh"); id.add(M_REFRESH);
            l.add("Settings"); id.add(M_SETTINGS);
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
            case M_REFRESH:
                forceAnim = true;
                loadDir(cwd, true);
                break;
            case M_SETTINGS:
                startActivity(new Intent(this, SettingsActivity.class));
                Ui.noSlideIfOff(this);
                break;
            case M_OPTIONS: {
                Item it = selectedPlainFile();
                if (it != null) {
                    File f = new File(cwd, it.name);
                    sel.clear();
                    redraw();
                    fileSheet(f, it);
                }
                break;
            }
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
                if (arc != null && arc.dirty) {
                    guardLeave(() -> onMenu(M_PLAIN));
                    break;
                }
                File dir = arc != null ? arc.file : new File(cwd, selectedSet().name);
                closeSession(arc);
                arc = null;
                sel.clear();
                loadDir(dir);
                break;
            }
            case M_SAVE: saveArchive(null); break;
            case M_DISCARD: discardArchive(null); break;
            case M_RENAME:
                if (arc != null) arcRename(); else renameSheet();
                break;
            case M_PROPS: propertiesSelected(); break;
            case M_ICON: {
                Item it = selectedDir();
                if (it != null) {
                    File f = new File(cwd, it.name);
                    sel.clear();
                    redraw();
                    iconPicker(f.getPath(), f.getName(), this::refreshDirIcons);
                }
                break;
            }
            case M_TEST: runTest(); break;
            case M_INFO: infoDialog(); break;
            case M_DECRYPT: decryptInPlace(); break;
            case M_EXTRACT_ALL: extractDialog(new ArrayList<String>()); break;
            default: break;
        }
    }

    /** Properties of what is selected: files and folders, or entries inside an open archive. */
    private void propertiesSelected() {
        if (sel.isEmpty()) return;
        if (arc != null) {
            List<String[]> picked = new ArrayList<>();
            for (String k : sel) {
                boolean dir = k.endsWith("/");
                long size = 0, files = 0, m = 0;
                for (Ent e : arc.ents) {
                    if (dir ? e.p.startsWith(k) : e.p.equals(k)) {
                        size += Math.max(e.s, 0);
                        files++;
                        m = Math.max(m, e.m);
                    }
                }
                picked.add(new String[]{k, String.valueOf(size), String.valueOf(m), dir ? "dir" : "file",
                        String.valueOf(files)});
            }
            Props.showInArchive(this, arc.file.getName(), picked);
            return;
        }
        Map<String, String> kinds = new HashMap<>();
        for (Item it : items) kinds.put(new File(it.name).getName(), it.kind);
        Props.show(this, selectedFiles(), kinds, f -> call("open_archive", f.getPath(), ""), null);
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
        sortList(rows);
        if (searching) sortList(srcRows);
    }

    private void sortList(List<Row> list) {
        final int key = sortKey;
        final boolean asc = sortAsc;
        Collections.sort(list, (a, b) -> {
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

    // ------------------------------------------------------------------ search

    private static List<String> terms(String q) {
        List<String> t = new ArrayList<>();
        for (String w : q.toLowerCase(Locale.ROOT).split("\\s+")) {
            if (!w.isEmpty()) t.add(w);
        }
        return t;
    }

    private static boolean matches(String name, List<String> t) {
        String l = name.toLowerCase(Locale.ROOT);
        for (String w : t) {
            if (!l.contains(w)) return false;
        }
        return true;
    }

    private void captureSource() {
        srcRows.clear();
        srcRows.addAll(rows);
        srcItems.clear();
        srcItems.addAll(items);
    }

    private void refreshList() {
        adapter.notifyDataSetChanged();
        updateEmpty();
    }

    private void showSource() {
        rows.clear();
        rows.addAll(srcRows);
        items.clear();
        items.addAll(srcItems);
        sel.retainAll(keysOf(rows));
        empty.setText(emptyText());
        redraw();
    }

    private void openSearch() {
        if (searching || app == null || !hasStorage()) return;
        deepOn = Defaults.deepSearch(this);
        searchField.setText("");
        searching = true;
        query = "";
        searchGen++;
        searchRow.setVisibility(View.VISIBLE);
        searchRow.animate().cancel();
        if (Defaults.animOn(this)) {
            searchRow.setAlpha(0f);
            searchRow.setTranslationY(-dp(10));
            searchRow.animate().alpha(1f).translationY(0).setDuration(Defaults.anim(this, 180)).start();
        } else {
            searchRow.setAlpha(1f);
            searchRow.setTranslationY(0);
        }
        renderSearchRow();
        renderHeader();
        searchField.requestFocus();
        showKeyboard(searchField);
    }

    /** Leaves search and shows the plain list again. */
    private void closeSearch() {
        if (!searching) return;
        closeSearchQuiet();
        showSource();
    }

    /** Leaves search without touching the list (the caller is about to replace it). */
    private void closeSearchQuiet() {
        if (!searching) return;
        searching = false;
        query = "";
        searchGen++;
        if (pendingSearch != null) {
            ui.removeCallbacks(pendingSearch);
            pendingSearch = null;
        }
        cancelDeep();
        deepRunning = false;
        hideKeyboard();
        searchRow.animate().cancel();
        searchRow.setVisibility(View.GONE);
        searchField.setText("");
    }

    private void renderSearchRow() {
        deepChip.setVisibility(arc == null ? View.VISIBLE : View.GONE);
        deepChip.setText(deepOn ? "Subfolders \u2713" : "Subfolders");
        deepChip.setTextColor(deepOn ? Ui.accent(this) : Ui.text2(this));
        deepChip.setBackground(deepOn
                ? Ui.pressable(this, getColor(R.color.k_chip), 0, 10)
                : Ui.pressable(this, 0, Ui.line(this), 10));
    }

    private void onQueryChanged(String text) {
        if (!searching) return;
        query = text.trim();
        searchGen++;
        if (pendingSearch != null) {
            ui.removeCallbacks(pendingSearch);
            pendingSearch = null;
        }
        cancelDeep();
        deepRunning = false;
        if (query.isEmpty()) {
            showSource();
            return;
        }
        filterLocal();                                      // instant: the list that's already here
        list.setSelection(0);
        refreshList();
        startDeepSoon(220);                                 // then the folders below, once typing pauses
    }

    /** Filters the folder (or the whole open archive) by the words in the box. */
    private void filterLocal() {
        List<String> t = terms(query);
        rows.clear();
        items.clear();
        if (arc != null) {
            for (Ent e : arc.ents) {
                if (rows.size() >= 1000) break;
                int sl = e.p.lastIndexOf('/');
                if (matches(e.p.substring(sl + 1), t)) rows.add(arcSearchRow(e, sl));
            }
        } else {
            for (Row r : srcRows) {
                if (matches(r.title, t)) {
                    rows.add(r);
                    if (r.item != null) items.add(r.item);
                }
            }
        }
        sortRows();
        sel.retainAll(keysOf(rows));
        empty.setText(rows.isEmpty() ? (deepRunning ? "Searching..." : "No matches.") : "");
    }

    private Row arcSearchRow(Ent e, int sl) {
        Row r = new Row();
        r.key = e.p;
        r.title = e.p.substring(sl + 1);
        r.ent = e;
        r.icon = Thumbs.iconFor(e.p);
        r.size = Math.max(e.s, 0);
        r.time = e.m;
        r.ext = Fmt.ext(r.title);
        String when = e.m > 0 ? Fmt.date(e.m) : "";
        String where;
        try {
            where = sl > 0 ? arcWhere(e.p.substring(0, sl)) : (Defaults.searchPath(this) == 1 ? "/" : "");
        } catch (RuntimeException ex) {
            where = "";                                     // a label must never crash the search
        }
        r.sub = where.isEmpty() ? when : (when.isEmpty() ? where : where + " \u00b7 " + when);
        r.right = Fmt.size(e.s);
        return r;
    }

    /** The folder shown under a search hit: below the searched folder, or the full path from /. */
    private String whereLabel(String relDir) {
        if (Defaults.searchPath(this) == 1) {
            return relDir.isEmpty() ? cwd.getPath() : new File(cwd, relDir).getPath();
        }
        return relDir.isEmpty() ? "This folder" : relDir;
    }

    /** Same for a hit inside an open archive. */
    private String arcWhere(String dir) {
        if (Defaults.searchPath(this) == 1) return "/" + dir;
        String d = dir + "/";
        if (arc != null && !arc.prefix.isEmpty() && d.startsWith(arc.prefix)) {
            // A hit directly in the folder being viewed: d equals the prefix, nothing below it.
            if (d.length() <= arc.prefix.length()) return "This folder";
            return d.substring(arc.prefix.length(), d.length() - 1);
        }
        return dir.isEmpty() ? "This folder" : dir;
    }

    private void startDeepSoon(long delay) {
        if (!searching || query.isEmpty() || arc != null || !deepOn) return;
        final int g = searchGen;
        if (pendingSearch != null) ui.removeCallbacks(pendingSearch);
        pendingSearch = () -> {
            pendingSearch = null;
            startDeep(g);
        };
        ui.postDelayed(pendingSearch, delay);
    }

    private void startDeep(final int g) {
        if (g != searchGen || !searching || app == null || arc != null) return;
        deepRunning = true;
        final String path = cwd.getPath(), q = query;
        final boolean all = showAll, hid = showHidden;
        if (rows.isEmpty()) {
            empty.setText("Searching...");
            updateEmpty();
        }
        new Thread(() -> {
            JSONObject r;
            try {
                r = Priv.wants(path) ? Priv.search(path, q, hid) : call("search", path, q, all, hid);
            } catch (Exception e) {
                r = errObj(String.valueOf(e.getMessage()));
            }
            final JSONObject res = r;
            ui.post(() -> {
                if (g != searchGen || !searching || arc != null) return;
                if (res.optBoolean("cancelled")) return;      // a newer search took over
                deepRunning = false;
                applyDeep(res);
            });
        }).start();
    }

    private void applyDeep(JSONObject res) {
        if (!res.optBoolean("ok")) {
            empty.setText(res.optString("error", "Search failed"));
            updateEmpty();
            return;
        }
        List<Row> rr = new ArrayList<>();
        List<Item> ii = new ArrayList<>();
        parseItems(res, rr, ii, true);
        rows.clear();
        rows.addAll(rr);
        items.clear();
        items.addAll(ii);
        sortRows();
        sel.retainAll(keysOf(rows));
        empty.setText("No matches.");
        if (res.optBoolean("truncated")) toast("Showing the first 1000 matches");
        refreshList();
    }

    private void cancelDeep() {
        final PyObject m = app;
        if (m == null || !deepRunning) return;
        new Thread(() -> {
            try {
                m.callAttr("search_cancel");
            } catch (Throwable ignored) {
            }
        }).start();
    }

    private void showKeyboard(final View v) {
        v.postDelayed(() -> {
            InputMethodManager m = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
            if (m != null) m.showSoftInput(v, 0);
        }, 60);
    }

    private void hideKeyboard() {
        View v = getCurrentFocus();
        InputMethodManager m = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
        if (v != null && m != null) m.hideSoftInputFromWindow(v.getWindowToken(), 0);
    }

    // ------------------------------------------------------------------ bookmarks

    /** The drop-down under the title: home, saved folders, and a button to save or drop this one. */
    private void showBookmarks() {
        if (marks != null && marks.isShowing()) {
            marks.dismiss();
            return;
        }
        if (arc != null || !hasStorage() || !sel.isEmpty()) return;
        final List<String> saved = Defaults.bookmarks(this);
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(dp(6), dp(6), dp(6), dp(6));
        card.setBackground(Ui.shape(this, Ui.surface(this), Ui.line(this), 18));
        volAt = -10000;
        card.addView(markRow(R.drawable.ic_home, "Internal storage", root.getPath(), false, root));
        for (File[] v : volumes()) {                              // USB drives and SD cards: listed while plugged in
            card.addView(markRow(R.drawable.ic_usb, v[1].getName(), v[0].getPath(), false, v[0]));
        }
        if (Priv.mode(this) != Priv.OFF) {                    // beyond what Android lets an app open
            card.addView(markRow(R.drawable.ic_d_phone, "Root (/)", "/", false, null));
            card.addView(markRow(R.drawable.ic_d_android, "Android/data",
                    new File(root, "Android/data").getPath(), false, null));
        }
        for (String bp : saved) {
            File f = new File(bp);
            card.addView(markRow(dirIcon(f), f.getName().isEmpty() ? bp : f.getName(), bp, true,
                    null));
        }
        if (!isStorageRoot(cwd)) {
            View d = new View(this);
            d.setBackgroundColor(Ui.line(this));
            LinearLayout.LayoutParams dl = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, dp(1));
            dl.topMargin = dp(4);
            dl.bottomMargin = dp(4);
            card.addView(d, dl);
            final String here = cwd.getPath();
            final boolean has = saved.contains(here);
            card.addView(actionRow(has ? R.drawable.ic_close : R.drawable.ic_add,
                    has ? "Remove this bookmark" : "Bookmark this folder", () -> {
                        if (has) {
                            Defaults.removeBookmark(this, here);
                            toast("Bookmark removed");
                        } else {
                            Defaults.addBookmark(this, here);
                            toast("Bookmarked " + cwd.getName());
                        }
                    }));
        }
        Ui.MaxScroll sv = new Ui.MaxScroll(this);
        sv.addView(card);
        FrameLayout wrap = new FrameLayout(this);
        wrap.setPadding(dp(8), dp(2), dp(8), dp(14));       // room for the shadow
        card.setElevation(dp(8));
        wrap.addView(sv);
        int w = Math.min(getResources().getDisplayMetrics().widthPixels - dp(8), dp(360));
        marks = new PopupWindow(wrap, w, ViewGroup.LayoutParams.WRAP_CONTENT, true);
        marks.setBackgroundDrawable(new ColorDrawable(0));
        marks.setOutsideTouchable(true);
        marks.setAnimationStyle(0);
        marks.showAsDropDown(titleBox, -titleBox.getLeft() - dp(4), 0);
        if (Defaults.animOn(this)) {
            wrap.setAlpha(0f);
            wrap.setScaleY(0.9f);
            wrap.setPivotX(dp(60));
            wrap.setPivotY(0f);
            wrap.animate().alpha(1f).scaleY(1f).setDuration(Defaults.anim(this, 170))
                    .setInterpolator(new android.view.animation.DecelerateInterpolator(1.6f)).start();
        }
    }

    /** Mounted removable volumes (flash drives, SD cards): {directory, label-as-File}. */
    private List<File[]> removableVolumes() {
        List<File[]> out = new ArrayList<>();
        try {
            android.os.storage.StorageManager sm =
                    (android.os.storage.StorageManager) getSystemService(STORAGE_SERVICE);
            for (android.os.storage.StorageVolume v : sm.getStorageVolumes()) {
                File d = v.getDirectory();
                if (d == null || v.isPrimary() || !v.isRemovable()) continue;
                if (!android.os.Environment.MEDIA_MOUNTED.equals(v.getState())) continue;
                String label = v.getDescription(this);
                out.add(new File[]{d, new File(label == null || label.isEmpty() ? d.getName() : label)});
            }
        } catch (Throwable ignored) {
        }
        return out;
    }

    private List<File[]> volCache = new ArrayList<>();
    private long volAt = -10000;

    /** Removable volumes, looked up at most every few seconds (this is asked for on every redraw). */
    private List<File[]> volumes() {
        long now = android.os.SystemClock.elapsedRealtime();
        if (now - volAt > 3000) {
            volCache = removableVolumes();
            volAt = now;
        }
        return volCache;
    }

    /** True for the top of any storage: Internal storage or a flash drive / SD card. Each one is
     *  its own home: no ".." row, and Back from here asks to exit instead of going somewhere else. */
    private boolean isStorageRoot(File d) {
        if (d.equals(root)) return true;
        if (d.getPath().equals("/")) return true;            // Root (/) when Root or Shizuku is on
        for (File[] v : volumes()) if (v[0].equals(d)) return true;
        return false;
    }

    /** The icon for a folder: Android's standard folders (Download, DCIM, Music...) get their own. */
    private int dirIcon(File d) {
        String picked = Defaults.folderIcon(this, d.getPath());
        if (picked != null) return Icons.byKey(picked);
        File p = d.getParentFile();
        return Icons.forDir(d, p != null && isStorageRoot(p));
    }

    /** The storage a folder lives on (Internal storage if it matches none). */
    private File storageRootOf(File f) {
        File best = root;
        String fp = f.getPath() + "/";
        int len = fp.startsWith(root.getPath() + "/") ? root.getPath().length() : -1;
        for (File[] v : volumes()) {
            String vp = v[0].getPath();
            if (fp.startsWith(vp + "/") && vp.length() > len) {
                best = v[0];
                len = vp.length();
            }
        }
        return best;
    }

    private String storageName(File r) {
        if (r.equals(root)) return "Internal storage";
        if (r.getPath().equals("/")) return "Root (/)";
        for (File[] v : volumes()) if (v[0].equals(r)) return v[1].getName();
        return r.getName();
    }

    /** {free, total} bytes of the storage holding f, or null. */
    private static long[] spaceOf(File f) {
        try {
            android.os.StatFs st = new android.os.StatFs(f.getPath());
            return new long[]{st.getAvailableBytes(), st.getTotalBytes()};
        } catch (Throwable t) {
            return null;
        }
    }

    /** After an icon was picked: give the folders on screen their new icon without reloading the list. */
    private void refreshDirIcons() {
        for (Row r : rows) if (r.item != null && "dir".equals(r.item.kind)) r.icon = dirIcon(new File(cwd, r.item.name));
        for (Row r : srcRows) if (r.item != null && "dir".equals(r.item.kind)) r.icon = dirIcon(new File(cwd, r.item.name));
        redraw();
    }

    /** A grid of folder icons to choose from, for a folder or a bookmark. `done` runs after a change. */
    private void iconPicker(final String path, String name, final Runnable done) {
        final Sheet s = new Sheet(this, "Icon for " + name);
        final String cur = Defaults.folderIcon(this, path);
        final int cols = 4;
        int accent = Ui.accent(this);
        LinearLayout grid = new LinearLayout(this);
        grid.setOrientation(LinearLayout.VERTICAL);
        LinearLayout line = null;
        int n = Icons.FOLDER_KEYS.length;
        for (int i = 0; i < n; i++) {
            if (i % cols == 0) {
                line = new LinearLayout(this);
                line.setOrientation(LinearLayout.HORIZONTAL);
                grid.addView(line, new LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
            }
            final String key = Icons.FOLDER_KEYS[i][0];
            boolean on = key.equals(cur);
            LinearLayout cell = new LinearLayout(this);
            cell.setOrientation(LinearLayout.VERTICAL);
            cell.setGravity(Gravity.CENTER_HORIZONTAL);
            cell.setPadding(dp(2), dp(8), dp(2), dp(6));
            cell.setBackground(Ui.pressable(this, on ? getColor(R.color.k_chip) : 0, on ? accent : 0, 12));
            ImageView iv = new ImageView(this);
            iv.setImageResource(Icons.byKey(key));
            Ui.tint(iv, accent);
            cell.addView(iv, new LinearLayout.LayoutParams(dp(40), dp(40)));
            TextView t = Ui.label(this, Icons.FOLDER_KEYS[i][1], 11, on, !on);
            t.setSingleLine(true);
            t.setEllipsize(TextUtils.TruncateAt.END);
            t.setGravity(Gravity.CENTER);
            cell.addView(t, new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
            cell.setOnClickListener(v -> {
                Defaults.setFolderIcon(this, path, key);
                s.dismiss();
                if (done != null) done.run();
            });
            LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            cp.setMargins(dp(2), dp(2), dp(2), dp(2));
            line.addView(cell, cp);
        }
        for (int i = n % cols; i != 0 && i < cols; i++) {            // keep the last row's cells the same width
            line.addView(new View(this), new LinearLayout.LayoutParams(0, 1, 1f));
        }
        s.content(grid);
        s.button("Use default", Ui.OUTLINE, () -> {
            Defaults.setFolderIcon(this, path, null);
            if (done != null) done.run();
            return true;
        });
        s.button("Cancel", Ui.TEXT, null);
        s.show();
    }

    private View iconTile(int icon) {
        ImageView iv = new ImageView(this);
        iv.setImageResource(icon);
        Ui.tint(iv, Ui.accent(this));
        iv.setPadding(dp(7), dp(7), dp(7), dp(7));
        iv.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 10));
        return iv;
    }

    private LinearLayout rowBase() {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(8), dp(7), dp(4), dp(7));
        row.setBackground(Ui.pressable(this, 0, 0, 12));
        return row;
    }

    /** One line of the bookmarks list. Storages show free/total on the right; a bookmarked folder
     *  shows nothing there (its size is never worked out: that meant walking the whole folder). */
    private View markRow(int icon, String name, final String path, boolean removable, File storage) {
        LinearLayout row = rowBase();
        LinearLayout.LayoutParams ip = new LinearLayout.LayoutParams(dp(36), dp(36));
        ip.rightMargin = dp(12);
        View tile = iconTile(icon);
        row.addView(tile, ip);
        if (removable) {                                      // a bookmark: its icon can be changed
            final String shown = name;
            View.OnClickListener pick = v -> {
                if (marks != null) marks.dismiss();
                iconPicker(path, shown, () -> {
                    if (arc == null && cwd != null) refreshDirIcons();
                    ui.post(this::showBookmarks);
                });
            };
            tile.setOnClickListener(pick);
            row.setOnLongClickListener(v -> { pick.onClick(v); return true; });
        }
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        TextView n = Ui.label(this, name, 15, path.equals(cwd.getPath()), false);
        if (path.equals(cwd.getPath())) n.setTextColor(Ui.accent(this));
        n.setSingleLine(true);
        n.setEllipsize(TextUtils.TruncateAt.END);
        TextView pt = Ui.label(this, path, 11, false, true);
        pt.setSingleLine(true);
        pt.setEllipsize(TextUtils.TruncateAt.START);
        col.addView(n);
        col.addView(pt);
        row.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        TextView right = Ui.label(this, "", 12, false, true);
        right.setGravity(Gravity.END);
        right.setPadding(dp(8), 0, 0, 0);
        if (storage != null) {
            long[] sp = spaceOf(storage);
            if (sp != null) right.setText(Fmt.size(sp[0]) + " free\nof " + Fmt.size(sp[1]));
        }
        row.addView(right, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));
        if (removable) {
            ImageButton x = Ui.iconButton(this, R.drawable.ic_close, v -> {
                Defaults.removeBookmark(this, path);
                if (marks != null) marks.dismiss();
                toast("Bookmark removed");
            });
            row.addView(x, new LinearLayout.LayoutParams(dp(40), dp(40)));
        } else {
            row.addView(new View(this), new LinearLayout.LayoutParams(dp(8), 1));
        }
        row.setOnClickListener(v -> {
            if (marks != null) marks.dismiss();
            goTo(path);
        });
        return row;
    }

    private View actionRow(int icon, String label, final Runnable r) {
        LinearLayout row = rowBase();
        LinearLayout.LayoutParams ip = new LinearLayout.LayoutParams(dp(36), dp(36));
        ip.rightMargin = dp(12);
        row.addView(iconTile(icon), ip);
        row.addView(Ui.label(this, label, 15, false, false),
                new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        row.setOnClickListener(v -> {
            if (marks != null) marks.dismiss();
            r.run();
        });
        return row;
    }

    private void goTo(final String path) {
        File f = new File(path);
        if (!f.isDirectory() && !(Priv.restricted(path) && Priv.mode(this) != Priv.OFF)) {
            toast("That folder isn't there any more");
            return;
        }
        if (arc != null && arc.dirty) {
            guardLeave(() -> goTo(path));
            return;
        }
        if (f.equals(cwd) && arc == null) return;
        loadDir(f);
    }

    // ------------------------------------------------------------------ Python and permissions

    private void startPython() {
        new Thread(() -> {
            try {
                if (!Python.isStarted()) Python.start(new AndroidPlatform(MainActivity.this));
                Python py = Python.getInstance();
                String helper = getApplicationInfo().nativeLibraryDir + "/libkiln_accel.so";
                String note = py.getModule("kiln_boot").callAttr("init", helper, getFilesDir().getPath()).toString();
                app = py.getModule("kiln_app");
                edit = py.getModule("kiln_edit");
                app.callAttr("set_jobs", Defaults.jobs(MainActivity.this));
                try {
                    app.callAttr("trim_cache", new File(getCacheDir(), "preview").getPath());
                } catch (Throwable ignored) {
                }
                try {
                    JSONObject sp = new JSONObject(app.callAttr("scan_params").toString());
                    byte[] sg = hexBytes(sp.getString("sig"));
                    sniffMin = sp.getInt("min");
                    sniffEnc = sp.getInt("enc");
                    sniffBuiltin = sp.getBoolean("builtin");
                    sniffIndex = sp.getString("index");
                    sniffSig = sg;                                   // last: it switches the fast scan on
                } catch (Throwable ignored) {
                    sniffSig = null;                                 // keep using the Python scan
                }
                ui.post(() -> {
                    if (!note.isEmpty()) Toast.makeText(MainActivity.this, note, Toast.LENGTH_LONG).show();
                    refreshPermission();
                    renderBars();
                    if (pendingIntent != null) {
                        Intent in = pendingIntent;
                        pendingIntent = null;
                        openIncoming(in);
                    }
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
        if (arc != null) {
            closeSession(arc);
            arcClip.clear();
        }
        boolean moved = arc != null || !dir.equals(cwd);
        if (moved && arc == null) rememberScroll();           // so Back returns to the same spot
        boolean goingUp = moved && (arc != null ? dir.equals(cwd) : isAncestor(dir, cwd));   // Back, up, or leaving an archive
        if (searching && moved) closeSearchQuiet();
        // A folder seen before is shown at once from memory and rescanned quietly behind it,
        // so going Back never waits for the scan. Otherwise the rows already on screen stay
        // until the new ones arrive.
        final Snap snap = moved ? snaps.get(snapKey(dir)) : null;
        if (snap == null && !rows.isEmpty()) upFrozen = upShown();   // the \"..\" row doesn't flicker meanwhile
        cwd = dir;
        arc = null;
        final int gen = ++loadGen;
        dirPending = snap == null;
        if (!keepSelection) sel.clear();
        if (snap != null) {
            upFrozen = null;
            plainSet = snap.plainSet;
            rows.clear();
            rows.addAll(snap.rows);
            items.clear();
            items.addAll(snap.items);
            sortRows();
            captureSource();
            empty.setText(emptyText());
            shownSig = snap.sig;
            lastDir = dir.getPath();
            redraw();
            // Scroll once, after the new rows are in, and let the slide-in start from the final
            // layout. (Scrolling twice re-laid the rows out after they'd started animating, so
            // the ones lower down skipped it.)
            if (goingUp) list.setSelectionFromTop(snap.first, snap.top);
            else list.setSelectionFromTop(0, 0);
            playRows();
        } else {
            if (moved) shownSig = null;                       // what's on screen isn't this folder
            if (rows.isEmpty()) {
                empty.setText("");                            // "Loading..." only if it's slow
                final String waiting = dir.getPath();
                ui.postDelayed(() -> {
                    if (dirPending && gen == loadGen && rows.isEmpty() && cwd.getPath().equals(waiting)
                            && empty.getText().length() == 0) {
                        empty.setText("Loading...");
                        updateEmpty();
                    }
                }, 700);
            }
            redraw();
        }
        final String path = dir.getPath();
        final boolean all = showAll, hidden = showHidden;
        new Thread(() -> {
            JSONObject r;
            try {
                r = null;
                if (Priv.restricted(path) && Priv.mode(MainActivity.this) != Priv.OFF) {
                    String why = Priv.denyReason(path);                  // Root or Shizuku does the listing
                    r = why != null ? errObj(why)
                            : Priv.scan(path, hidden, sniffSig, sniffMin, sniffBuiltin, sniffEnc, sniffIndex);
                } else if (sniffSig != null) {
                    try {
                        r = scanFast(path, all, hidden);
                    } catch (Throwable ignored) {
                        r = null;                                    // fall back to the Python scan
                    }
                }
                if (r == null) r = call("scan", path, all, hidden);
            } catch (Exception e) {
                r = errObj(String.valueOf(e.getMessage()));
            }
            if (!r.optBoolean("ok") && Priv.restricted(path) && Priv.mode(MainActivity.this) == Priv.OFF) {
                r = errObj(r.optString("error", "Couldn't read this folder")
                        + "\nTurn on Root or Shizuku in Settings > Access to open it.");
            }
            final JSONObject res = r;
            ui.post(() -> {
                if (gen != loadGen || arc != null || !cwd.getPath().equals(path)) return;
                dirPending = false;
                upFrozen = null;
                applyScan(res);
            });
        }).start();
    }

    /** True when `dir` is a folder above `below` (the parent, or further up). */
    private static boolean isAncestor(File dir, File below) {
        String d = dir.getPath(), b = below.getPath();
        return !d.equals(b) && b.startsWith(d.endsWith("/") ? d : d + "/");
    }

    private int loadGen;
    private boolean dirPending;                                // a folder is loading: rows are the old ones
    private Boolean upFrozen;

    private String lastDir = "";

    // ---- folders seen before, kept in memory so Back is instant

    private static final class Snap {
        List<Row> rows;
        List<Item> items;
        boolean plainSet;
        String sig;
        int first, top;                                        // where the list was scrolled to
    }

    private final java.util.LinkedHashMap<String, Snap> snaps =
            new java.util.LinkedHashMap<String, Snap>(32, 0.75f, true) {
                @Override
                protected boolean removeEldestEntry(java.util.Map.Entry<String, Snap> e) {
                    return size() > 64;
                }
            };
    private String shownSig;                                   // what the rows on screen were built from

    private String snapKey(File dir) { return dir.getPath() + "|" + showAll + "|" + showHidden; }

    private void rememberScroll() {
        if (searching || dirPending || shownSig == null) return;
        Snap s = snaps.get(snapKey(cwd));
        if (s == null) return;
        s.first = list.getFirstVisiblePosition();
        View c = list.getChildAt(0);
        s.top = c == null ? 0 : c.getTop() - list.getPaddingTop();
    }

    private static String sigOf(List<Item> its, boolean set) {
        StringBuilder b = new StringBuilder(its.size() * 40).append(set).append('\n');
        for (Item it : its) {
            b.append(it.name).append('|').append(it.kind).append('|').append(it.size).append('|')
                    .append(it.mtime).append('|').append(it.enc).append('\n');   // counts arrive separately
        }
        return b.toString();
    }

    private String emptyText() {
        if (arc != null) return "This folder is empty.";
        return showAll ? "This folder is empty."
                : "No Kiln archives here.\nOpen the menu to show all files.";
    }

    // ---- fast folder scan (Java): list, tell archives from other files, count folder items later

    private volatile byte[] sniffSig;                         // null until Python says what to look for
    private int sniffMin = 25, sniffEnc = 1;
    private boolean sniffBuiltin = true;
    private String sniffIndex = "_index.kln";
    private static final java.util.concurrent.ExecutorService POOL =
            java.util.concurrent.Executors.newFixedThreadPool(4, r -> {
                Thread t = new Thread(r, "kiln-scan");
                t.setDaemon(true);
                return t;
            });

    private static byte[] hexBytes(String h) {
        byte[] b = new byte[h.length() / 2];
        for (int i = 0; i < b.length; i++) b[i] = (byte) Integer.parseInt(h.substring(2 * i, 2 * i + 2), 16);
        return b;
    }

    /** One entry as the same JSON the Python scan returns, or null to leave it out. Folder counts are -1 (filled in later). */
    private JSONObject scanEntry(String dir, String name, boolean all) {
        try {
            String full = dir.endsWith("/") ? dir + name : dir + "/" + name;
            android.system.StructStat st = android.system.Os.stat(full);
            long mtime = st.st_mtime;
            JSONObject o = new JSONObject();
            if (android.system.OsConstants.S_ISDIR(st.st_mode)) {
                boolean isSet = new File(full, sniffIndex).isFile();
                o.put("n", name).put("k", isSet ? "set" : "dir").put("s", 0).put("m", mtime).put("c", -1);
                return o;
            }
            if (!android.system.OsConstants.S_ISREG(st.st_mode)) return null;
            String kind = "file";
            boolean enc = false;
            byte[] sig = sniffSig;
            long size = st.st_size;
            if (size >= sniffMin) {
                byte[] head = new byte[Math.max(5, sig.length + 1)];
                int n = 0;
                java.io.FileDescriptor fd = null;
                try {
                    fd = android.system.Os.open(full, android.system.OsConstants.O_RDONLY, 0);
                    n = android.system.Os.read(fd, head, 0, head.length);
                } catch (Exception ignored) {
                    n = 0;
                } finally {
                    if (fd != null) {
                        try { android.system.Os.close(fd); } catch (Exception ignored) { }
                    }
                }
                if (n >= sig.length && startsWith(head, sig)) {
                    if (sniffBuiltin) {
                        if (n >= 5) { kind = "archive"; enc = (head[4] & sniffEnc) != 0; }
                    } else {
                        kind = "archive";
                    }
                } else if (n >= 4 && head[0] == 'K' && head[1] == 'L' && head[2] == 'N'
                        && (head[3] == '9' || head[3] == '2')) {
                    kind = "old";
                }
            }
            if (kind.equals("file") && !all) return null;
            o.put("n", name).put("k", kind).put("s", size).put("m", mtime).put("e", enc);
            return o;
        } catch (Exception e) {
            return null;
        }
    }

    private static boolean startsWith(byte[] a, byte[] p) {
        if (a.length < p.length) return false;
        for (int i = 0; i < p.length; i++) if (a[i] != p[i]) return false;
        return true;
    }

    /** Lists a folder natively. Returns null when it can't (the Python scan then reports why). */
    private JSONObject scanFast(final String path, final boolean all, boolean hidden) throws Exception {
        String[] names = new File(path).list();
        if (names == null) return null;
        final List<String> keep = new ArrayList<>(names.length);
        for (String n : names) {
            if (hidden || !n.startsWith(".")) keep.add(n);       // hidden entries cost nothing
        }
        final JSONObject[] got = new JSONObject[keep.size()];
        if (keep.size() >= 64) {                                   // storage waits: read several at once
            List<java.util.concurrent.Callable<Void>> jobs = new ArrayList<>();
            int chunk = (keep.size() + 3) / 4;
            for (int s0 = 0; s0 < keep.size(); s0 += chunk) {
                final int from = s0, to = Math.min(keep.size(), s0 + chunk);
                jobs.add(() -> {
                    for (int i = from; i < to; i++) got[i] = scanEntry(path, keep.get(i), all);
                    return null;
                });
            }
            for (java.util.concurrent.Future<Void> f : POOL.invokeAll(jobs)) f.get();
        } else {
            for (int i = 0; i < got.length; i++) got[i] = scanEntry(path, keep.get(i), all);
        }
        List<JSONObject> items = new ArrayList<>(got.length);
        for (JSONObject o : got) if (o != null) items.add(o);
        Collections.sort(items, (x, y) -> {
            boolean fx = !x.optString("k").equals("archive") && !x.optString("k").equals("old")
                    && !x.optString("k").equals("file");
            boolean fy = !y.optString("k").equals("archive") && !y.optString("k").equals("old")
                    && !y.optString("k").equals("file");
            if (fx != fy) return fx ? -1 : 1;
            return x.optString("n").compareToIgnoreCase(y.optString("n"));
        });
        JSONArray arr = new JSONArray();
        for (JSONObject o : items) arr.put(o);
        JSONObject r = new JSONObject();
        r.put("ok", true).put("path", path).put("items", arr)
                .put("is_set", new File(path, sniffIndex).isFile());
        return r;
    }

    private final Map<String, Integer> countCache = new HashMap<>();     // "folder/sub" -> items in it

    private void giveCount(Item it, Row row, int c) {
        it.count = c;
        row.size = Math.max(c, 0);
        row.right = c < 0 ? "" : plural(c, "item", "items");
    }

    /** Folder item counts take a listing of each subfolder, so they arrive after the list is shown. */
    private void loadCounts(final String path, final int gen, boolean force) {
        final List<String> need = new ArrayList<>();
        for (Item it : items) {
            if (it.kind.equals("dir") && (force || it.count < 0)) need.add(it.name);
        }
        if (need.isEmpty()) return;
        if (Priv.restricted(path) && Priv.mode(this) != Priv.OFF) return;   // counting would run a command per folder
        new Thread(() -> {
            final Map<String, Integer> got = new HashMap<>();
            List<java.util.concurrent.Callable<Void>> jobs = new ArrayList<>();
            final java.util.concurrent.ConcurrentHashMap<String, Integer> out = new java.util.concurrent.ConcurrentHashMap<>();
            int chunk = Math.max(16, (need.size() + 3) / 4);
            for (int s0 = 0; s0 < need.size(); s0 += chunk) {
                final int from = s0, to = Math.min(need.size(), s0 + chunk);
                jobs.add(() -> {
                    for (int i = from; i < to; i++) {
                        String[] l = new File(path, need.get(i)).list();
                        out.put(need.get(i), l == null ? 0 : l.length);
                    }
                    return null;
                });
            }
            try {
                for (java.util.concurrent.Future<Void> f : POOL.invokeAll(jobs)) f.get();
            } catch (Exception e) {
                return;
            }
            got.putAll(out);
            ui.post(() -> {
                if (gen != loadGen || arc != null || searching || !cwd.getPath().equals(path)) return;
                boolean changed = false;
                for (Row r : rows) {
                    if (r.item == null || !r.item.kind.equals("dir")) continue;
                    Integer c = got.get(r.item.name);
                    if (c == null) continue;
                    countCache.put(path + "/" + r.item.name, c);
                    if (r.item.count != c) {
                        giveCount(r.item, r, c);
                        changed = true;
                    }
                }
                if (countCache.size() > 20000) countCache.clear();
                if (!changed) return;
                if (sortKey == S_SIZE) sortRows();
                adapter.notifyDataSetChanged();
            });
        }).start();
    }

    private void applyScan(JSONObject r) {
        swipe.setRefreshing(false);
        boolean isSet = r.optBoolean("is_set");
        if (!r.optBoolean("ok")) {
            rows.clear();
            items.clear();
            plainSet = isSet;
            shownSig = null;
            empty.setText(r.optString("error", "Couldn't read this folder"));
            redraw();
            return;
        }
        List<Row> nr = new ArrayList<>();
        List<Item> ni = new ArrayList<>();
        parseItems(r, nr, ni, false);
        for (int i = 0; i < ni.size() && i < nr.size(); i++) {      // counts seen before show at once
            Item it = ni.get(i);
            if (it.kind.equals("dir") && it.count < 0) {
                Integer known = countCache.get(cwd.getPath() + "/" + it.name);
                giveCount(it, nr.get(i), known == null ? -1 : known);
            }
        }
        String sig = sigOf(ni, isSet);
        boolean newDir = !cwd.getPath().equals(lastDir);
        lastDir = cwd.getPath();
        boolean replay = newDir || forceAnim;                  // a new folder, or Refresh: slide the rows in
        forceAnim = false;
        boolean same = !replay && sig.equals(shownSig) && arc == null
                && !(searching && !query.isEmpty());
        shownSig = sig;
        Snap sn = snaps.get(snapKey(cwd));
        if (sn == null) {
            sn = new Snap();
            snaps.put(snapKey(cwd), sn);
        }
        sn.rows = new ArrayList<>(nr);
        sn.items = new ArrayList<>(ni);
        sn.plainSet = isSet;
        sn.sig = sig;
        if (same) {
            loadCounts(cwd.getPath(), loadGen, false);
            return;                                            // nothing changed: the list isn't touched
        }
        plainSet = isSet;
        rows.clear();
        rows.addAll(nr);
        items.clear();
        items.addAll(ni);
        sel.retainAll(keysOf(rows));
        sortRows();
        captureSource();
        empty.setText(emptyText());
        if (searching && !query.isEmpty()) {
            filterLocal();
            startDeepSoon(0);
        }
        if (newDir) list.setSelectionFromTop(0, 0);            // a new folder always starts at the top
        redraw();
        if (replay) playRows();
        loadCounts(cwd.getPath(), loadGen, replay);
    }

    /** Turns scan or search output into rows. Names from a search are paths below the folder. */
    private void parseItems(JSONObject r, List<Row> outRows, List<Item> outItems, boolean hits) {
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
            outItems.add(it);
            Row row = new Row();
            row.key = it.name;
            int slash = it.name.lastIndexOf('/');
            row.title = slash >= 0 ? it.name.substring(slash + 1) : it.name;
            row.item = it;
            row.time = it.mtime;
            row.ext = Fmt.ext(it.name);
            String when = Fmt.date(it.mtime);
            if (hits) row.where = whereLabel(slash > 0 ? it.name.substring(0, slash) : "");   // search: say where
            switch (it.kind) {
                case "dir":
                    row.folder = true;
                    row.size = Math.max(it.count, 0);
                    row.icon = dirIcon(new File(cwd, it.name));
                    row.sub = when;
                    row.right = it.count < 0 ? "" : plural(it.count, "item", "items");
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
                    row.icon = Thumbs.iconFor(it.name);
                    row.path = new File(cwd, it.name).getPath();
                    row.sub = when;
                    row.right = Fmt.size(it.size);
                    break;
            }
            outRows.add(row);
        }
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
        if (dirPending) return;                              // still showing the previous folder
        Row r = rowAt(pos);
        if (r == null) {                                     // the ".." row
            if (sel.isEmpty()) onBackPressed();
            return;
        }
        if (!sel.isEmpty()) {
            toggle(r.key);
            return;
        }
        if (arc != null) {
            if (r.dir) {
                arcEnter(r.key);
            } else {
                arcFileSheet(r);
            }
            return;
        }
        Item it = r.item;
        if (pickArc != null && !it.kind.equals("dir")) {      // picking files to add: tap selects
            toggle(r.key);
            return;
        }
        switch (it.kind) {
            case "dir": loadDir(new File(cwd, it.name)); break;
            case "archive":
            case "set": openItem(it, null); break;
            case "old":
                message("Older format", "This archive was made by an older Kiln (format 9 or earlier) "
                        + "that this app can't open.");
                break;
            default: openRegular(new File(cwd, it.name), it); break;
        }
    }

    /** A file in a protected folder is copied to the cache first; everything else opens as before. */
    private void openRegular(final File f, final Item it) {
        if (!Priv.wants(f.getPath())) {
            openFile(f, it);
            return;
        }
        stageThen(f, st -> openFile(st, it));
    }

    private void stageThen(final File f, final OnFile cb) {
        final ProgressUi pu = new ProgressUi("Preparing...", null);
        new Thread(() -> {
            File st = null;
            String err = null;
            try {
                st = Priv.stage(f.getPath());
            } catch (Exception e) {
                err = String.valueOf(e.getMessage());
            }
            final File got = st;
            final String why = err;
            ui.post(() -> {
                pu.close();
                if (got == null) message("Couldn't open " + f.getName(), why);
                else cb.got(got);
            });
        }).start();
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

    private Item selectedPlainFile() {
        for (Item it : items) {
            if (sel.contains(it.name) && it.kind.equals("file")) return it;
        }
        return null;
    }

    private Item selectedDir() {
        for (Item it : items) {
            if (sel.contains(it.name) && it.kind.equals("dir")) return it;
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

    /** Tapping a file: use the app chosen for its extension, otherwise ask. */
    private void openFile(File f, Item it) {
        String def = Defaults.get(this, Fmt.ext(f.getName()));
        if (def == null) {
            fileSheet(f, it);
            return;
        }
        if (Defaults.KILN_VIEW.equals(def)) {
            if (kindOf(f.getName()) == 2) showImage(f, false);
            else openText(f, false);
        } else if (Defaults.KILN_EDIT.equals(def)) {
            openText(f, true);
        } else if (!launch(f, false, Defaults.pkgOf(def))) {
            Defaults.clear(this, Fmt.ext(f.getName()));       // the app is gone: forget it
            toast("That app isn't available any more");
            fileSheet(f, it);
        }
    }

    private void fileSheet(final File f, Item it) {
        int kind = kindOf(f);
        Sheet s = new Sheet(this, f.getName());
        s.text(Fmt.size(it.size) + " \u00b7 " + Fmt.date(it.mtime));
        boolean small = it.size <= TextActivity.MAX_BYTES;
        if (kind == 2 && it.size <= IMAGE_PREVIEW_MAX) {
            s.option(R.drawable.ic_image, "View in Kiln", false, () -> showImage(f, false));
        } else if (kind == 1 && small) {
            s.option(R.drawable.ic_open, "View in Kiln", false, () -> openText(f, false));
            s.option(R.drawable.ic_edit, "Edit in Kiln", false, () -> openText(f, true));
        } else if (kind == 0 && small) {
            s.option(R.drawable.ic_open, "View as text in Kiln", false, () -> openText(f, false));
            s.option(R.drawable.ic_edit, "Edit as text in Kiln", false, () -> openText(f, true));
        }
        s.divider();
        s.option(R.drawable.ic_open, "Open with...", false, () -> appChooser(f, false));
        s.option(R.drawable.ic_edit, "Edit with...", false, () -> appChooser(f, true));
        s.button("Cancel", Ui.TEXT, null).show();
    }

    private void openText(File f, boolean edit) {
        needRefresh = edit;
        Intent i = new Intent(this, TextActivity.class);
        i.putExtra("path", f.getPath());
        i.putExtra("edit", edit);
        startActivity(i);
        Ui.noSlideIfOff(this);
    }

    private static String mimeOf(File f) {
        String m = MimeTypeMap.getSingleton().getMimeTypeFromExtension(Fmt.ext(f.getName()));
        if (m != null) return m;
        if (kindOf(f.getName()) == 1) return "text/plain";
        return Sniff.mime(f);          // no extension, or one Android doesn't know: go by the content
    }

    private Intent viewIntent(File f, boolean edit) {
        Uri uri = FileProvider.getUriForFile(this, getPackageName() + ".files", f);
        if (edit) Priv.watch(f.getPath());                     // a staged copy: written back when we return
        Intent i = new Intent(edit ? Intent.ACTION_EDIT : Intent.ACTION_VIEW);
        i.setDataAndType(uri, mimeOf(f));
        i.setClipData(ClipData.newRawUri("", uri));
        i.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION
                | (edit ? Intent.FLAG_GRANT_WRITE_URI_PERMISSION : 0));
        return i;
    }

    /** Opens f in one specific app. False if that app can't take it. */
    private boolean launch(File f, boolean edit, String pkg) {
        try {
            Intent i = viewIntent(f, edit);
            i.setPackage(pkg);
            needRefresh = edit;
            startActivity(i);
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    private void openExternal(File f, boolean edit) { appChooser(f, edit); }

    /** Our own "Open with" list: app icons and names, and an "always" tick for this extension. */
    private void appChooser(final File f, final boolean edit) {
        final PackageManager pm = getPackageManager();
        List<ResolveInfo> found;
        try {
            found = pm.queryIntentActivities(viewIntent(f, edit), PackageManager.MATCH_DEFAULT_ONLY);
        } catch (Exception e) {
            toast("No app can open this file");
            return;
        }
        final String ext = Fmt.ext(f.getName());
        final CheckBox always = Ui.check(this, ext.isEmpty() ? "Always open this with the app"
                : "Always open ." + ext + " files with this app", false);
        final Sheet s = new Sheet(this, edit ? "Edit with" : "Open with");
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        int kind = kindOf(f);
        boolean small = f.length() <= TextActivity.MAX_BYTES;
        if (!edit && (kind == 2 || ((kind == 1 || kind == 0) && small))) {
            box.addView(appRow(getDrawable(R.drawable.ic_open), "Kiln " + (kind == 2 ? "image viewer" : "text viewer"), () -> {
                if (always.isChecked()) Defaults.set(this, ext, Defaults.KILN_VIEW);
                s.dismiss();
                if (kind == 2) showImage(f, false);
                else openText(f, false);
            }));
        }
        if (!edit && (kind == 1 || kind == 0) && small) {
            box.addView(appRow(getDrawable(R.drawable.ic_edit), "Kiln text editor", () -> {
                if (always.isChecked()) Defaults.set(this, ext, Defaults.KILN_EDIT);
                s.dismiss();
                openText(f, true);
            }));
        }
        java.util.Set<String> seen = new java.util.HashSet<>();
        for (final ResolveInfo ri : found) {
            final String pkg = ri.activityInfo.packageName;
            if (pkg.equals(getPackageName()) || !seen.add(pkg)) continue;
            final String label = String.valueOf(ri.loadLabel(pm));
            box.addView(appRow(ri.loadIcon(pm), label, () -> {
                s.dismiss();
                if (launch(f, edit, pkg)) {
                    if (always.isChecked() && !edit) Defaults.set(this, ext, pkg + "|" + label);
                } else {
                    toast("That app couldn't open the file");
                }
            }));
        }
        box.addView(appRow(getDrawable(R.drawable.ic_more), "More options...", () -> {
            s.dismiss();
            try {
                needRefresh = edit;
                startActivity(Intent.createChooser(viewIntent(f, edit), edit ? "Edit with" : "Open with"));
            } catch (Exception e) {
                toast("No app can open this file");
            }
        }));
        s.content(box);
        if (!edit && !ext.isEmpty()) s.pinned(always);          // stays in view while the app list scrolls
        s.button("Cancel", Ui.TEXT, null).show();
    }

    private View appRow(Drawable icon, String label, final Runnable r) {
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(8), dp(8), dp(8), dp(8));
        row.setBackground(Ui.pressable(this, 0, 0, 12));
        ImageView iv = new ImageView(this);
        iv.setImageDrawable(icon);
        if (icon != null && icon.getConstantState() != null && label.startsWith("Kiln")
                || label.equals("More options...")) {
            iv.setImageTintList(ColorStateList.valueOf(Ui.accent(this)));
            iv.setPadding(dp(7), dp(7), dp(7), dp(7));
        }
        row.addView(iv, new LinearLayout.LayoutParams(dp(38), dp(38)));
        TextView t = Ui.label(this, label, 16, false, false);
        t.setSingleLine(true);
        t.setEllipsize(TextUtils.TruncateAt.END);
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tp.leftMargin = dp(14);
        row.addView(t, tp);
        row.setOnClickListener(v -> r.run());
        return row;
    }

    private void showImage(final File f, final boolean cleanup) {
        Intent in = new Intent(this, ImageActivity.class);
        in.putExtra(ImageActivity.EXTRA_PATH, f.getPath());
        in.putExtra(ImageActivity.EXTRA_CLEANUP, cleanup);    // a temp copy from an archive: delete when done
        startActivity(in);
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
            File use = f;
            try {
                if (Priv.wants(f.getPath())) use = Priv.stage(f.getPath());   // work on a copy in the cache
                r = call("open_archive", use.getPath(), pw);
            } catch (Exception e) {
                r = errObj(String.valueOf(e.getMessage()));
            }
            final JSONObject res = r;
            final File opened = use;
            ui.post(() -> {
                pu.close();
                onOpened(opened, pw, res, after);
            });
        }).start();
    }

    private void onOpened(final File f, final String pw, JSONObject r, final Runnable after) {
        if (!r.optBoolean("ok")) {
            if (r.optBoolean("need_password") || r.optBoolean("bad_password")) {
                if (r.optBoolean("bad_password")) toast("Incorrect password");
                askPassword(f.getName(), p -> doOpen(f, p, after));
            } else {
                message("Couldn't open " + f.getName(), r.optString("error", "Unknown error"));
            }
            return;
        }
        Arc a = new Arc();
        a.file = f;
        a.home = f.equals(incomingFile) ? incomingHome : null;
        String real = Priv.originOf(f.getPath());              // opened from a protected folder
        if (real != null) a.home = new File(real).getParentFile();
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
        closeSearchQuiet();
        rememberScroll();                                     // so closing the archive returns to this spot
        closeSession(arc);
        arcClip.clear();
        arc = a;
        plainSet = false;
        sel.clear();
        buildArcRows();
        list.setSelectionFromTop(0, 0);
        if (after != null) after.run();
    }

    private File incomingFile, incomingHome;

    /** A .kln handed to us by another app (ZArchiver, a browser, a share...). */
    private void openIncoming(final Intent intent) {
        final Uri uri = intent.getData();
        if (uri == null) return;
        if (arc != null && arc.dirty) {
            guardLeave(() -> openIncoming(intent));
            return;
        }
        if ("file".equals(uri.getScheme()) && uri.getPath() != null && new File(uri.getPath()).isFile()) {
            File f = new File(uri.getPath());
            incomingFile = f;
            incomingHome = f.getParentFile();
            doOpen(f, "", null);
            return;
        }
        String n = null;
        try (Cursor c = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME},
                null, null, null)) {
            if (c != null && c.moveToFirst()) n = c.getString(0);
        } catch (Exception ignored) {
        }
        if (n == null || n.isEmpty()) n = uri.getLastPathSegment();
        if (n == null || n.isEmpty()) n = "archive.kln";
        n = n.replace('/', '_');
        final File dir = new File(getCacheDir(), "incoming");
        File[] older = dir.listFiles();                           // copies of earlier ones are not needed
        if (older != null) {
            for (File o : older) {
                if (arc == null || !o.equals(arc.file)) deleteTree(o);
            }
        }
        dir.mkdirs();
        final File out = new File(dir, n);
        runOp("Opening...", true, p -> {
            boolean good = false;
            long total = -1;
            try (Cursor c = getContentResolver().query(uri, new String[]{OpenableColumns.SIZE},
                    null, null, null)) {
                if (c != null && c.moveToFirst() && !c.isNull(0)) total = c.getLong(0);
            } catch (Exception ignored) {
            }
            try (InputStream in = getContentResolver().openInputStream(uri);
                 OutputStream os = new FileOutputStream(out)) {
                if (in == null) throw new java.io.IOException("can't read that file");
                byte[] buf = new byte[1 << 16];
                int k;
                long done = 0, lastEv = 0;
                while ((k = in.read(buf)) > 0) {
                    if (p.cancelled()) break;
                    os.write(buf, 0, k);
                    done += k;
                    long now = System.currentTimeMillis();
                    if (total > 0 && now - lastEv > 100) {
                        lastEv = now;
                        p.event("{\"frac\":" + Math.min(1.0, (double) done / total)
                                + ",\"file\":" + JSONObject.quote(out.getName()) + "}");
                    }
                }
                good = !p.cancelled();
            } finally {
                if (!good) out.delete();                          // never keep a half copy
            }
            JSONObject ok = new JSONObject();
            ok.put("ok", good);
            ok.put("cancelled", !good);
            return ok;
        }, r -> {
            if (r.optBoolean("cancelled")) {
                toast("Cancelled");
                return;
            }
            if (!r.optBoolean("ok")) {
                message("Couldn't open the file", r.optString("message", r.optString("error")));
                return;
            }
            incomingFile = out;
            incomingHome = root;
            doOpen(out, "", null);
        });
    }

    private void closeArchive() {
        if (arc != null && incomingFile != null && arc.file.equals(incomingFile)) {
            final File gone = incomingFile;                       // the private copy of a file opened from
            incomingFile = null;                                  // another app: free the space now
            new Thread(() -> deleteTree(gone)).start();
        }
        closeSession(arc);
        arcClip.clear();
        sel.clear();
        loadDir(cwd);                                         // loadDir sees the open archive and leaves it
    }

    /** Where the list is scrolled to: {first visible row, its offset from the top}. */
    private int[] scrollNow() {
        View c = list.getChildAt(0);
        return new int[]{list.getFirstVisiblePosition(), c == null ? 0 : c.getTop() - list.getPaddingTop()};
    }

    /** Open a folder inside the archive: remember where this one was, start the new one at the top. */
    private void arcEnter(String prefix) {
        if (!searching) arc.pos.put(arc.prefix, scrollNow());
        arc.prefix = prefix;
        buildArcRows();
        list.setSelectionFromTop(0, 0);
    }

    /** Back to a folder inside the archive: land where it was left. */
    private void arcBackTo(String prefix) {
        arc.prefix = prefix;
        buildArcRows();
        final int[] at = arc.pos.get(prefix);
        final Arc a = arc;
        list.setSelectionFromTop(at == null ? 0 : at[0], at == null ? 0 : at[1]);
        if (at != null) list.post(() -> {                       // once the rows are laid out
            if (arc == a && a.prefix.equals(prefix)) list.setSelectionFromTop(at[0], at[1]);
        });
    }

    private void buildArcRows() {
        dirPending = false;                                   // an archive view replaces any folder load
        upFrozen = null;
        shownSig = null;                                      // these rows are not a folder listing
        loadGen++;
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
            r.icon = Thumbs.iconFor(e.p);
            r.size = Math.max(e.s, 0);
            r.time = e.m;
            r.ext = Fmt.ext(r.title);
            r.sub = "+".equals(e.k) ? "New" + (e.m > 0 ? " \u00b7 " + Fmt.date(e.m) : "")
                    : "~".equals(e.k) ? "Renamed" : e.m > 0 ? Fmt.date(e.m) : "";
            r.right = Fmt.size(e.s);
            rows.add(r);
        }
        sortRows();
        captureSource();
        empty.setText(emptyText());
        if (searching && !query.isEmpty()) filterLocal();
        redraw();
    }


    // ------------------------------------------------------------------ editing inside an archive
    // Changes are staged in a kiln_edit session (nothing is written until Save).

    private JSONObject ecall(String fn, Object... args) throws Exception {
        PyObject m = edit;
        if (m == null) throw new IllegalStateException("Python is still starting");
        return new JSONObject(m.callAttr(fn, args).toString());
    }

    private void closeSession(Arc a) {
        if (a == null || a.sid == null) return;
        final String sid = a.sid;
        a.sid = null;
        a.dirty = false;
        new Thread(() -> {
            try {
                ecall("close", sid);
            } catch (Exception ignored) {
            }
        }).start();
    }

    /** Starts the staged-edit session the first time something is changed. */
    private void withSession(final Runnable then) {
        final Arc a = arc;
        if (a == null) return;
        if (a.sid != null) {
            then.run();
            return;
        }
        runOp("Preparing...", true, p -> {
            JSONObject r = ecall("begin", a.file.getPath(), a.password);
            if (p.cancelled() && r.optBoolean("ok")) {            // cancelled while it was loading
                try {
                    ecall("close", r.optString("sid"));
                } catch (Exception ignored) {
                }
                JSONObject c = new JSONObject();
                c.put("ok", false);
                c.put("cancelled", true);
                return c;
            }
            return r;
        }, r -> {
            if (r.optBoolean("cancelled")) {
                toast("Cancelled");
                return;
            }
            if (!r.optBoolean("ok")) {
                message("Can't edit this archive", r.optString("message", r.optString("error")));
                return;
            }
            if (arc != a) {                                   // left the archive while it was loading
                try {
                    ecall("close", r.optString("sid"));
                } catch (Exception ignored) {
                }
                return;
            }
            a.sid = r.optString("sid");
            applyEdit(r);
            then.run();
        });
    }

    /** Takes the archive's current staged contents from a kiln_edit result. */
    private void applyEdit(JSONObject r) {
        if (arc == null) return;
        JSONArray arr = r.optJSONArray("entries");
        if (arr != null) {
            arc.ents.clear();
            for (int i = 0; i < arr.length(); i++) {
                JSONObject o = arr.optJSONObject(i);
                if (o == null) continue;
                Ent e = new Ent();
                e.p = o.optString("p");
                e.s = o.optLong("s");
                e.m = o.optLong("m");
                e.k = o.optString("k");
                arc.ents.add(e);
            }
        }
        arc.dirty = r.optBoolean("dirty", arc.dirty);
        sel.clear();
        buildArcRows();
    }

    private void editOp(String heading, final Job job) {
        runOp(heading, false, job, r -> {
            if (r.optBoolean("ok")) applyEdit(r);
            else message("Couldn't do that", r.optString("message", r.optString("error")));
        });
    }

    private static String trimSlash(String s) {
        return s.endsWith("/") ? s.substring(0, s.length() - 1) : s;
    }

    private View saveRow() {
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(12), dp(4), dp(4), dp(0));
        TextView t = Ui.label(this, "Unsaved changes", 14, true, false);
        row.addView(t, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        row.addView(Ui.button(this, "Discard", Ui.TEXT, v -> discardArchive(null)));
        Button save = Ui.button(this, "Save", Ui.FILLED, v -> saveArchive(null));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.leftMargin = dp(6);
        row.addView(save, lp);
        return row;
    }

    private void saveArchive(final Runnable after) {
        final Arc a = arc;
        if (a == null || a.sid == null) {
            if (after != null) after.run();
            return;
        }
        runOp("Saving...", true, p -> {
            JSONObject sr = ecall("save", a.sid, p);
            String real = Priv.originOf(a.file.getPath());
            if (real != null && sr.optBoolean("ok")) {            // write the saved copy back to the real place
                try {
                    Priv.push(a.file, real);
                } catch (Exception e) {
                    sr = errObj("Saved here, but couldn't write it back: " + e.getMessage());
                }
            }
            return sr;
        }, r -> {
            if (r.optBoolean("cancelled")) {
                toast("Cancelled");
            } else if (r.optBoolean("ok")) {
                needRefresh = true;
                snaps.clear();                                // the file on disk changed
                if (arc == a) applyEdit(r);
                else a.dirty = false;
                toast("Saved");
                if (after != null) after.run();
            } else {
                message("Couldn't save", r.optString("message", r.optString("error")));
            }
        });
    }

    private void discardArchive(final Runnable after) {
        final Arc a = arc;
        if (a == null || a.sid == null) {
            if (after != null) after.run();
            return;
        }
        confirm("Discard changes?", "Everything you changed since the last save is thrown away.",
                "Discard", Ui.DANGER, () -> {
                    arcClip.clear();
                    bg(p -> ecall("discard", a.sid), r -> {
                        if (r.optBoolean("ok")) {
                            if (arc == a) applyEdit(r);
                            else a.dirty = false;
                            if (after != null) after.run();
                        } else {
                            message("Couldn't discard", r.optString("message", r.optString("error")));
                        }
                    });
                });
    }

    /** Runs `then` now, or after asking what to do with unsaved changes. */
    private void guardLeave(final Runnable then) {
        if (arc == null || !arc.dirty) {
            then.run();
            return;
        }
        new Sheet(this, "Unsaved changes")
                .text("This archive has changes that haven't been saved.")
                .button("Keep editing", Ui.OUTLINE, null)
                .button("Discard", Ui.DANGER, () -> {
                    final Arc a = arc;
                    closeSession(a);
                    if (a != null) a.dirty = false;
                    then.run();
                    return true;
                })
                .button("Save", Ui.FILLED, () -> {
                    saveArchive(then);
                    return true;
                }).show();
    }

    private void arcDelete() {
        final List<String> keys = new ArrayList<>(sel);
        if (keys.isEmpty()) return;
        final int n = countFor(keys);
        confirm("Delete " + plural(n, "file", "files") + "?",
                "They are removed from the archive when you save. Discard brings them back.",
                "Delete", Ui.DANGER, () -> withSession(() -> {
                    final Arc a = arc;
                    editOp("Deleting...", p -> ecall("remove", a.sid, membersJson(keys)));
                }));
    }

    private void arcRename() {
        if (sel.size() != 1) return;
        final String key = sel.iterator().next();
        final boolean dir = key.endsWith("/");
        final String full = trimSlash(key);
        final String old = full.substring(full.lastIndexOf('/') + 1);
        final String parent = full.contains("/") ? full.substring(0, full.lastIndexOf('/')) : "";
        nameSheet("Rename", old, "Rename", n -> {
            if (n.equals(old)) return null;
            withSession(() -> {
                final Arc a = arc;
                final List<String> keys = new ArrayList<>();
                keys.add(key);
                arcTransfer(a, keys, parent, true, "ask", n);
            });
            return null;
        });
    }

    private void arcSetClip(boolean move) {
        arcClip.clear();
        arcClip.addAll(sel);
        arcClipMove = move;
        sel.clear();
        redraw();
    }

    private View arcPasteBar(int accent) {
        LinearLayout info = new LinearLayout(this);
        info.setOrientation(LinearLayout.VERTICAL);
        info.setPadding(dp(12), dp(8), dp(12), dp(2));
        info.addView(Ui.label(this, (arcClipMove ? "Moving " : "Copying ")
                + plural(arcClip.size(), "item", "items"), 15, true, false));
        String where = arc.prefix.isEmpty() ? arc.file.getName()
                : arc.prefix.substring(0, arc.prefix.length() - 1);
        info.addView(Ui.label(this, "Into " + where, 12, false, true));
        bottomBar.addView(info);
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(2), dp(4), dp(4), dp(2));
        row.addView(tile(R.drawable.ic_new_folder, "New folder", accent, this::arcNewFolder),
                new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT));
        row.addView(new View(this), new LinearLayout.LayoutParams(0, 1, 1f));
        row.addView(Ui.button(this, "Cancel", Ui.TEXT, v -> {
            arcClip.clear();
            redraw();
        }));
        Button go = Ui.button(this, arcClipMove ? "Move here" : "Paste here", Ui.FILLED,
                v -> arcPaste());
        LinearLayout.LayoutParams gp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        gp.leftMargin = dp(6);
        row.addView(go, gp);
        bottomBar.addView(row);
        return row;
    }

    private void arcPaste() {
        final List<String> keys = new ArrayList<>(arcClip);
        final boolean mv = arcClipMove;
        withSession(() -> arcTransfer(arc, keys, trimSlash(arc.prefix), mv, "ask", null));
    }

    private void arcTransfer(final Arc a, final List<String> keys, final String dest,
                             final boolean mv, final String policy, final String renameTo) {
        runOp(mv ? "Moving..." : "Copying...", false,
                p -> ecall("transfer", a.sid, membersJson(keys), dest, mv, policy, renameTo),
                r -> {
                    if (r.optBoolean("ok")) {
                        arcClip.clear();
                        applyEdit(r);
                        return;
                    }
                    JSONArray c = r.optJSONArray("conflicts");
                    if (c == null || c.length() == 0) {
                        message("Couldn't " + (mv ? "move" : "copy"),
                                r.optString("message", r.optString("error")));
                        return;
                    }
                    StringBuilder names = new StringBuilder();
                    for (int i = 0; i < c.length() && i < 5; i++) {
                        names.append("\u2022 ").append(c.optString(i)).append('\n');
                    }
                    int total = r.optInt("n_conflicts", c.length());
                    if (total > 5) names.append("and ").append(total - 5).append(" more\n");
                    new Sheet(this, plural(total, "item already exists", "items already exist"))
                            .text(names.toString().trim())
                            .option("Replace", false, () -> arcTransfer(a, keys, dest, mv, "replace", renameTo))
                            .option("Skip existing", false, () -> arcTransfer(a, keys, dest, mv, "skip", renameTo))
                            .option("Keep both", false, () -> arcTransfer(a, keys, dest, mv, "keep", renameTo))
                            .button("Cancel", Ui.TEXT, null).show();
                });
    }

    /** Archives store files only: a new folder is just a place to put things (gone if left empty). */
    private void arcNewFolder() {
        nameSheet("New folder", "", "Create", n -> {
            arcEnter(arc.prefix + n + "/");
            return null;
        });
    }

    private void arcNewSheet() {
        new Sheet(this, "Add")
                .option(R.drawable.ic_add, "Files or folders from phone", false, this::startPick)
                .option(R.drawable.ic_new_folder, "New folder", false, this::arcNewFolder)
                .option(R.drawable.ic_new_file, "Text file", false, () ->
                        nameSheet("New text file", "untitled.txt", "Create", n -> {
                            for (Ent e : arc.ents) {
                                if (e.p.equals(arc.prefix + n)) return "That name is taken";
                            }
                            final String member = arc.prefix + n;
                            withSession(() -> {
                                final Arc a = arc;
                                runOp("Creating...", false, p -> ecall("new_text", a.sid, member), r -> {
                                    if (!r.optBoolean("ok")) {
                                        message("Couldn't create it", r.optString("message"));
                                        return;
                                    }
                                    applyEdit(r);
                                    editInKiln(member);
                                });
                            });
                            return null;
                        }))
                .button("Cancel", Ui.TEXT, null).show();
    }

    // ---- picking phone files to add

    private void startPick() {
        withSession(() -> {
            pickArc = arc;
            pickReturn = cwd;
            pickDest = trimSlash(arc.prefix);
            arc = null;
            sel.clear();
            loadDir(cwd);
            toast("Long-press to select folders; tap files");
        });
    }

    private void pickBar(boolean selecting, int accent) {
        LinearLayout info = new LinearLayout(this);
        info.setOrientation(LinearLayout.VERTICAL);
        info.setPadding(dp(12), dp(8), dp(12), dp(2));
        info.addView(Ui.label(this, "Adding to " + pickArc.file.getName()
                + (pickDest.isEmpty() ? "" : "/" + pickDest), 15, true, false));
        info.addView(Ui.label(this, selecting ? plural(sel.size(), "item", "items") + " selected"
                : "Tap files, long-press folders", 12, false, true));
        bottomBar.addView(info);
        LinearLayout row = new LinearLayout(this);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(2), dp(4), dp(4), dp(2));
        row.addView(new View(this), new LinearLayout.LayoutParams(0, 1, 1f));
        row.addView(Ui.button(this, "Cancel", Ui.TEXT, v -> endPick(false)));
        Button go = Ui.button(this, "Add", Ui.FILLED, v -> endPick(true));
        go.setEnabled(selecting);
        LinearLayout.LayoutParams gp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        gp.leftMargin = dp(6);
        row.addView(go, gp);
        bottomBar.addView(row);
    }

    private void endPick(boolean doAdd) {
        final Arc a = pickArc;
        if (a == null) return;
        final List<File> files = doAdd ? selectedFiles() : new ArrayList<File>();
        final String dest = pickDest;
        pickArc = null;
        sel.clear();
        cwd = pickReturn != null ? pickReturn : cwd;
        arc = a;
        buildArcRows();
        if (!files.isEmpty()) {
            editOp("Adding...", p -> ecall("add", a.sid, pathsJson(files), dest));
        }
    }

    // ---- editing a file's contents

    private void editInKiln(final String member) {
        stageForEdit(member, false);
    }

    private void editExternalFile(final String member) {
        stageForEdit(member, true);
    }

    private void stageForEdit(final String member, final boolean external) {
        withSession(() -> {
            final Arc a = arc;
            final File cache = new File(getCacheDir(), "edit");
            cache.mkdirs();
            runOp("Preparing...", true, p -> ecall("file_for", a.sid, member, cache.getPath(), p, false), r -> {
                if (r.optBoolean("cancelled")) {
                    toast("Cancelled");
                    return;
                }
                if (!r.optBoolean("ok")) {
                    message("Couldn't open", r.optString("message", r.optString("error")));
                    return;
                }
                File f = new File(r.optString("file"));
                editFile = f;
                editMember = member;
                editStamp = f.lastModified() * 31 + f.length();
                editExternal = external;
                if (external) appChooser(f, true);
                else openText(f, true);
            });
        });
    }

    /** Back from an editor: stage the edited copy into the archive if it changed. */
    private void checkEditedFile() {
        final File f = editFile;
        if (f == null || arc == null || arc.sid == null) return;
        if (!f.exists()) {
            editFile = null;
            return;
        }
        long now = f.lastModified() * 31 + f.length();
        if (now == editStamp) return;
        editFile = null;
        final String member = editMember;
        final Arc a = arc;
        final Runnable stage = () -> editOp("Updating...", p -> ecall("set_file", a.sid, member, f.getPath()));
        if (editExternal) {
            confirm("Update the archive?", "\"" + member.substring(member.lastIndexOf('/') + 1)
                    + "\" was changed. Put the new version in the archive? (You still need to Save.)",
                    "Update", Ui.FILLED, stage);
        } else {
            stage.run();
        }
    }

    // ------------------------------------------------------------------ progress and jobs

    /** Modal progress sheet. cancelP == null hides the Cancel button. */
    private final class ProgressUi {
        final Sheet sheet;
        final ProgressBar bar;
        final TextView line, pct;

        ProgressUi(String heading, final Progress cancelP) {
            line = Ui.label(MainActivity.this, "", 14, false, true);
            line.setSingleLine(true);
            line.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            pct = Ui.label(MainActivity.this, "", 16, true, false);
            bar = new ProgressBar(MainActivity.this, null, android.R.attr.progressBarStyleHorizontal);
            bar.setIndeterminate(true);
            bar.setMax(1000);
            ColorStateList acc = ColorStateList.valueOf(Ui.accent(MainActivity.this));
            bar.setProgressTintList(acc);
            bar.setIndeterminateTintList(acc);
            LinearLayout box = new LinearLayout(MainActivity.this);
            box.setOrientation(LinearLayout.VERTICAL);
            LinearLayout top = new LinearLayout(MainActivity.this);       // file name ... percent
            top.setGravity(Gravity.CENTER_VERTICAL);
            top.addView(line, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            LinearLayout.LayoutParams pp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            pp.leftMargin = dp(12);
            top.addView(pct, pp);
            box.addView(top);
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            bp.topMargin = dp(12);
            box.addView(bar, bp);
            CheckBox awake = Ui.check(MainActivity.this, "Keep the screen from turning off", false);
            awake.setOnCheckedChangeListener((v, on) -> {
                if (on) getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                else getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            });
            LinearLayout.LayoutParams ap = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            ap.topMargin = dp(10);
            box.addView(awake, ap);
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

        // The engine reports only when a chunk is finished, which can be seconds apart on a phone.
        // So the bar eases toward each report and, between reports, creeps forward at the speed
        // measured so far (slowing as it nears where the next report is expected, never past it).
        private double target, shown, rate, ceiling;     // fractions 0..1; rate = fraction per ms
        private long lastReport = android.os.SystemClock.uptimeMillis(), lastTick = lastReport;
        private boolean closed, indet = true, ticking;
        private String lastFile = "";
        private final Runnable tick = new Runnable() {
            @Override public void run() {
                if (closed) return;
                long now = android.os.SystemClock.uptimeMillis();
                long dt = Math.min(now - lastTick, 100);
                lastTick = now;
                if (shown < target) {                                // catch up to the last report
                    shown += (target - shown) * Math.min(1.0, dt / 140.0);
                    if (target - shown < 0.0004) shown = target;
                } else if (target < 1.0 && rate > 0 && ceiling > target) {   // creep toward the next
                    double room = ceiling - target;
                    double p = Math.min(1.0, (shown - target) / room);
                    shown = Math.min(ceiling - room * 0.02, shown + rate * 0.9 * (1.0 - p) * dt);
                }
                if (!indet) paint();
                ui.postDelayed(this, 33);
            }
        };

        private void paint() {
            bar.setProgress((int) Math.round(shown * 1000));
            int pc = target >= 1.0 && shown >= 0.999 ? 100 : (int) Math.floor(shown * 100);
            pct.setText(Math.min(100, pc) + "%");
        }

        void update(double frac, String file) {
            if (!file.isEmpty() && !file.equals(lastFile)) {
                lastFile = file;
                line.setText(file);
            }
            if (frac < 0) {
                if (!indet) {
                    indet = true;
                    bar.setIndeterminate(true);
                    pct.setText("");
                }
                return;
            }
            if (indet) {
                indet = false;
                bar.setIndeterminate(false);
            }
            long now = android.os.SystemClock.uptimeMillis();
            if (frac > target) {
                double step = frac - target;
                double r = step / Math.max(now - lastReport, 1);
                rate = rate == 0 ? r : rate * 0.5 + r * 0.5;
                ceiling = Math.min(1.0, frac + step);
                target = frac;
                lastReport = now;
            }
            if (!ticking) {
                ticking = true;
                lastTick = now;
                ui.post(tick);
            }
        }

        void close() {
            closed = true;
            ui.removeCallbacks(tick);
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            sheet.dismiss();
        }
    }

    private long notifAt;
    private int notifPct = -2;
    private String notifText = "";

    void onProgress(String json) {
        try {
            JSONObject o = new JSONObject(json);
            final double frac = o.optDouble("frac", -1);
            final String file = o.optString("file", "");
            ui.post(() -> {
                if (currentProgress != null) currentProgress.update(frac, file);
            });
            if (svcOn) {
                // the notification is rebuilt on every call: only send real changes, at most
                // twice a second, so it can't slow the work down or flicker
                final int pc = frac >= 0 ? (int) Math.round(frac * 100) : -1;
                final String txt = file.isEmpty() ? "Working..." : file;
                long now = android.os.SystemClock.uptimeMillis();
                if ((pc != notifPct || !txt.equals(notifText)) && (now - notifAt >= 500 || pc >= 100)) {
                    notifAt = now;
                    notifPct = pc;
                    notifText = txt;
                    OpService.progress(this, txt, pc);
                }
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
        notifAt = 0;
        notifPct = -2;
        notifText = "";
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
        toast("Open a folder, then tap \"" + (move ? "Move here" : "Paste here") + "\"");
    }

    private void paste() {
        final List<File> srcs = new ArrayList<>(clip);
        final boolean mv = clipMove;
        final File dest = cwd;
        final String json = pathsJson(srcs);
        bg(p -> Priv.involves(json, dest.getPath()) ? Priv.plan(json, dest.getPath(), mv)
                : call("fs_plan", json, dest.getPath(), mv), r -> {
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
                p -> Priv.involves(json, dest.getPath()) ? Priv.transfer(json, dest.getPath(), mv, policy)
                        : call("fs_transfer", json, dest.getPath(), mv, policy, p), r -> {
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
                runOp("Deleting...", true, p -> Priv.involves(pathsJson(victims), "") ? Priv.delete(pathsJson(victims))
                        : call("fs_delete", pathsJson(victims), p), r -> {
                    if (r.optBoolean("cancelled")) toast("Cancelled");
                    else if (!r.optBoolean("ok")) message("Couldn't delete everything", r.optString("message"));
                    loadDir(cwd);
                }));
    }

    // ------------------------------------------------------------------ new, rename

    private void newSheet() {
        new Sheet(this, "New")
                .option(R.drawable.ic_new_folder, "Folder", false, this::newFolderHere)
                .option(R.drawable.ic_new_file, "Text file", false, () ->
                        nameSheet("New text file", "untitled.txt", "Create", n -> {
                            if (Priv.wants(cwd.getPath())) {
                                String er = Priv.createFile(new File(cwd, n).getPath());
                                if (er != null) return er;
                                loadDir(cwd);
                                return null;
                            }
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

    /** Makes a folder in the folder being shown. Also works while items are being held to paste. */
    private void newFolderHere() {
        nameSheet("New folder", "", "Create", n -> {
            if (Priv.wants(cwd.getPath())) {
                String er = Priv.mkdirs(new File(cwd, n).getPath());
                if (er != null) return er;
                loadDir(cwd);
                return null;
            }
            File nf = new File(cwd, n);
            if (nf.exists()) return "That name is taken";
            if (!nf.mkdirs()) return "Couldn't create it";
            loadDir(cwd);
            return null;
        });
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
            if (Priv.wants(cwd.getPath())) {
                File pf = new File(cwd, old), pt = new File(cwd, n);
                String er = Priv.rename(pf.getPath(), pt.getPath());
                if (er != null) return er;
                Defaults.moveFolderIcons(this, pf.getPath(), pt.getPath());
                loadDir(cwd);
                return null;
            }
            File target = new File(cwd, n);
            if (target.exists()) return "That name is taken";
            File from = new File(cwd, old);
            if (!from.renameTo(target)) return "Couldn't rename";
            Defaults.moveFolderIcons(this, from.getPath(), target.getPath());
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
        // every row gets an icon, like the file sheet outside an archive; groups are divided
        if (text || image) s.option(image ? R.drawable.ic_image : R.drawable.ic_open,
                kind == 0 ? "View as text in Kiln" : "View in Kiln", false, () -> previewFile(r.ent.p, 0));
        if (text) s.option(R.drawable.ic_edit, "Edit as text in Kiln", false,
                () -> editInKiln(r.ent.p));
        if (text || image) s.divider();
        s.option(R.drawable.ic_open, "Open with...", false, () -> previewFile(r.ent.p, 1));
        s.option(R.drawable.ic_edit, "Edit with...", false, () -> editExternalFile(r.ent.p));
        if (!arc.dirty) {
            s.divider();
            s.option(R.drawable.ic_extract, "Extract...", false, () -> {
                List<String> k = new ArrayList<>();
                k.add(r.key);
                extractDialog(k);
            });
        }
        s.button("Cancel", Ui.TEXT, null).show();
    }

    /** Extracts one file to the cache (or finds it there already), then shows it (mode 0) or
     *  hands it to another app (1). The extraction can be cancelled; nothing partial stays. */
    private void previewFile(final String member, final int mode) {
        final Arc a = arc;
        final long size = sizeOf(member);
        final File cache = new File(getCacheDir(), "preview");
        cache.mkdirs();
        runOp("Opening...", true,
                p -> a.sid != null ? ecall("file_for", a.sid, member, cache.getPath(), p, true)
                        : call("preview", a.file.getPath(), a.password, member, cache.getPath(), size, p),
                r -> {
                    if (r.optBoolean("cancelled")) {
                        toast("Cancelled");
                        return;
                    }
                    if (!r.optBoolean("ok")) {
                        message("Couldn't open", r.optString("message"));
                        return;
                    }
                    File f = new File(r.optString("file"));
                    int kind = mode == 1 ? 0 : kindOf(f);         // looks inside names it doesn't know
                    if (mode == 1) {
                        openExternal(f, false);
                    } else if (kind == 2) {
                        showImage(f, false);                      // the cache keeps it for next time
                    } else if (kind == -1) {
                        new Sheet(this, "Not a text file").text("\"" + f.getName() + "\" looks like "
                                + "binary data, so Kiln can't show it as text. You can open it in another app.")
                                .button("Cancel", Ui.TEXT, null)
                                .button("Open with...", Ui.FILLED, () -> {
                                    openExternal(f, false);
                                    return true;
                                }).show();
                    } else {
                        openText(f, false);
                    }
                });
    }

    /** Size of a file inside the open archive (0 if unknown). */
    private long sizeOf(String member) {
        if (arc == null) return 0;
        for (Ent e : arc.ents) if (e.p.equals(member)) return e.s;
        return 0;
    }

    private void extractDialog(final List<String> keys) {
        if (arc != null && arc.dirty) {
            message("Unsaved changes", "Save or discard your changes first, then extract.");
            return;
        }
        final Arc a = arc;
        File defDest = new File(a.home != null ? a.home : a.file.getParentFile(),
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
                p -> {
                    if (!Priv.wants(dest)) {
                        return call("extract", a.file.getPath(), a.password, membersJson(keys), dest,
                                overwrite, total, p);
                    }
                    File tmp = Priv.newTemp();                    // extract here, then copy into the protected folder
                    try {
                        JSONObject xr = call("extract", a.file.getPath(), a.password, membersJson(keys),
                                tmp.getPath(), true, total, p);
                        if (xr.optBoolean("ok")) {
                            String er = Priv.pushInto(tmp, dest, overwrite);
                            if (er != null) xr = errObj("Couldn't copy into " + dest + ": " + er);
                        }
                        return xr;
                    } finally {
                        Priv.deleteTree(tmp);
                    }
                },
                r -> {
                    String msg = r.optString("message");
                    if (r.optBoolean("ok")) {
                        sel.clear();
                        redraw();
                        extractResult(a, dest, msg, overwrite);
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

    /** The result of Extract: a header, the numbers as tiles, where it went, and Open folder. */
    private void extractResult(Arc a, final String dest, String msg, boolean overwrite) {
        int tone = Ui.accent(this);
        LinearLayout head = new LinearLayout(this);
        head.setGravity(Gravity.CENTER_VERTICAL);
        ImageView ic = new ImageView(this);
        ic.setImageResource(R.drawable.ic_extract);
        Ui.tint(ic, tone);
        ic.setPadding(dp(13), dp(13), dp(13), dp(13));
        ic.setBackground(Ui.shape(this, Ui.alpha(tone, 0x26), 0, 16));
        head.addView(ic, new LinearLayout.LayoutParams(dp(54), dp(54)));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(this, "Extraction complete", 18, true, false));
        TextView nm = Ui.label(this, a.file.getName(), 13, false, true);
        nm.setSingleLine(true);
        nm.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        col.addView(nm);
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        cp.leftMargin = dp(14);
        head.addView(col, cp);
        Sheet sh = new Sheet(this, null).content(head).space(16);

        List<String[]> rows = new ArrayList<>();
        // kiln's line: "extracted 2 file(s), 88 B in 0.0s - all checksums OK"
        //          or: "extracted 5 file(s) from /path in 0.4s - all checksums OK" (no size)
        java.util.regex.Matcher m = java.util.regex.Pattern.compile(
                "(\\d+) file\\(s\\)(?: from [^\\n]+?)?(?:, ([^\\n]+?))? in ([\\d.]+)s").matcher(msg);
        boolean verified = msg.contains("all checksums OK");
        if (m.find()) {
            int n = Integer.parseInt(m.group(1));
            LinearLayout stats = new LinearLayout(this);
            if (m.group(2) != null) {
                stats.addView(statTile(String.valueOf(n), n == 1 ? "file" : "files"), statLp(false));
                stats.addView(statTile(m.group(2), "extracted"), statLp(true));
                stats.addView(statTile(m.group(3) + " s", "took"), statLp(false));
            } else {                                    // no size in the line: two wider tiles
                stats.addView(statTile(String.valueOf(n), n == 1 ? "file" : "files"), Ui.tileLp(this, false, true));
                stats.addView(statTile(m.group(3) + " s", "took"), Ui.tileLp(this, true, false));
            }
            sh.content(stats).space(16);
        } else {
            rows.add(new String[]{"Result", msg.isEmpty() ? "Done" : msg, "stack"});
        }
        rows.add(new String[]{"Saved to", dest, "stack"});
        rows.add(new String[]{"From", a.file.getName(), "stack"});
        if (verified) rows.add(new String[]{"Verification", "Every checksum matches"});
        if (overwrite) rows.add(new String[]{"Existing files", "Replaced"});
        sh.content(detailCard(rows)).space(4)
                .button("OK", Ui.TEXT, null)
                .button("Open folder", Ui.FILLED, () -> {
                    arc = null;
                    loadDir(new File(dest));
                    return true;
                }).show();
    }

    private void runTest() {
        if (arc != null && arc.dirty) {
            message("Unsaved changes", "Save your changes first; Check works on the saved archive.");
            return;
        }
        final Arc a = arc;
        runOp("Checking...", true, p -> call("test", a.file.getPath(), a.password, p), r -> {
            if (r.optBoolean("ok")) {
                checkResult(a, true, r.optString("message"));
            } else if (r.optBoolean("cancelled")) {
                toast("Cancelled");
            } else {
                checkResult(a, false, r.optString("message"));
            }
        });
    }

    /** The result of Check: a clear verdict, the numbers as tiles, and the details underneath. */
    private void checkResult(Arc a, boolean ok, String msg) {
        int tone = ok ? Ui.accent(this) : getColor(R.color.k_danger);
        LinearLayout head = new LinearLayout(this);
        head.setGravity(Gravity.CENTER_VERTICAL);
        ImageView ic = new ImageView(this);
        ic.setImageResource(ok ? R.drawable.ic_check : R.drawable.ic_close);
        Ui.tint(ic, tone);
        ic.setPadding(dp(13), dp(13), dp(13), dp(13));
        ic.setBackground(Ui.shape(this, Ui.alpha(tone, 0x26), 0, 16));
        head.addView(ic, new LinearLayout.LayoutParams(dp(54), dp(54)));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(this, ok ? "Archive is intact" : "Problem found", 18, true, false));
        TextView nm = Ui.label(this, a.file.getName(), 13, false, true);
        nm.setSingleLine(true);
        nm.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        col.addView(nm);
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        cp.leftMargin = dp(14);
        head.addView(col, cp);
        Sheet sh = new Sheet(this, null).content(head).space(16);

        List<String[]> rows = new ArrayList<>();
        // kiln's success line: "Checked 12 file(s), 3.4 MB in 0.8s - all checksums OK"
        java.util.regex.Matcher m = ok ? java.util.regex.Pattern
                .compile("(\\d+) file\\(s\\), (.+?) in ([\\d.]+)s").matcher(msg) : null;
        if (m != null && m.find()) {
            LinearLayout stats = new LinearLayout(this);
            int n = Integer.parseInt(m.group(1));
            stats.addView(statTile(String.valueOf(n), n == 1 ? "file checked" : "files checked"), statLp(false));
            stats.addView(statTile(m.group(2), "of data"), statLp(true));
            stats.addView(statTile(m.group(3) + " s", "took"), statLp(false));
            sh.content(stats).space(16);
            rows.add(new String[]{"Result", "Every checksum matches"});
        } else if (ok) {
            rows.add(new String[]{"Result", msg.isEmpty() ? "No problems found" : msg, "stack"});
        } else {
            String[] lines = msg.isEmpty() ? new String[]{"The check failed."} : msg.split("\n");
            rows.add(new String[]{"What went wrong", lines[0].trim(), "stack"});
            if (lines.length > 1) {
                StringBuilder more = new StringBuilder();
                for (int i = 1; i < lines.length; i++) {
                    if (lines[i].trim().isEmpty()) continue;
                    if (more.length() > 0) more.append('\n');
                    more.append(lines[i].trim());
                }
                if (more.length() > 0) rows.add(new String[]{"Details", more.toString(), "stack"});
            }
        }
        rows.add(new String[]{"Encryption", a.encrypted ? "AES-256-GCM" : "None"});
        rows.add(new String[]{"Files in archive", String.valueOf(a.ents.size())});
        if (!ok) {
            TextView hint = Ui.label(this, "The archive may be damaged. Keep your original files until "
                    + "you've restored or re-created it.", 13, false, true);
            sh.content(detailCard(rows)).space(12).content(hint);
        } else {
            sh.content(detailCard(rows));
        }
        sh.space(4).button("OK", Ui.FILLED, null).show();
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
        final Arc a = arc;
        long raw = 0;
        java.util.HashSet<String> folders = new java.util.HashSet<>();
        for (Ent e : a.ents) {
            raw += Math.max(e.s, 0);
            for (int i = e.p.indexOf('/'); i >= 0; i = e.p.indexOf('/', i + 1)) folders.add(e.p.substring(0, i));
        }

        // header: an icon for what kind of archive it is, the name, and whether it's locked
        LinearLayout head = new LinearLayout(this);
        head.setGravity(Gravity.CENTER_VERTICAL);
        ImageView ic = new ImageView(this);
        ic.setImageResource(a.isSet ? R.drawable.ic_set : a.encrypted ? R.drawable.ic_lock : R.drawable.ic_archive);
        Ui.tint(ic, Ui.accent(this));
        ic.setPadding(dp(13), dp(13), dp(13), dp(13));
        ic.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 16));
        head.addView(ic, new LinearLayout.LayoutParams(dp(54), dp(54)));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        TextView name = Ui.label(this, a.file.getName(), 18, true, false);
        name.setMaxLines(2);
        name.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        col.addView(name);
        TextView lock = Ui.label(this, a.encrypted ? "Encrypted \u00b7 AES-256-GCM" : "Not encrypted", 13, false, true);
        col.addView(lock);
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        cp.leftMargin = dp(14);
        head.addView(col, cp);

        // three numbers at a glance
        LinearLayout stats = new LinearLayout(this);
        stats.addView(statTile(String.valueOf(a.ents.size()), a.ents.size() == 1 ? "file" : "files"), statLp(false));
        stats.addView(statTile(Fmt.size(raw), "original"), statLp(true));
        stats.addView(statTile(Fmt.size(a.packed), "stored"), statLp(false));

        Sheet sh = new Sheet(this, null).content(head).space(16).content(stats);

        // how much the packing saved: a bar of stored size against the original
        if (raw > 0) {
            double ratio = Math.min(1.0, (double) a.packed / raw);
            long saved = Math.max(raw - a.packed, 0);
            LinearLayout cmp = new LinearLayout(this);
            cmp.setOrientation(LinearLayout.VERTICAL);
            LinearLayout top = new LinearLayout(this);
            top.addView(Ui.label(this, "Compression", 13, false, true),
                    new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            TextView right = Ui.label(this, saved > 0
                    ? "Saved " + Fmt.size(saved) + " (" + Math.round(100.0 * saved / raw) + "%)"
                    : "No saving", 13, true, false);
            right.setTextColor(Ui.accent(this));
            top.addView(right);
            cmp.addView(top);
            ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
            bar.setMax(1000);
            bar.setProgress((int) Math.max(Math.round(ratio * 1000), 8));   // never an empty-looking bar
            bar.setProgressTintList(ColorStateList.valueOf(Ui.accent(this)));
            bar.setProgressBackgroundTintList(ColorStateList.valueOf(Ui.line(this)));
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, dp(8));
            bp.topMargin = dp(8);
            cmp.addView(bar, bp);
            TextView cap = Ui.label(this, "Stored size is " + Math.round(100.0 * a.packed / raw)
                    + "% of the original", 12, false, true);
            LinearLayout.LayoutParams capP = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            capP.topMargin = dp(6);
            cmp.addView(cap, capP);
            sh.space(16).content(cmp);
        }

        List<String[]> rows = new ArrayList<>();
        rows.add(new String[]{"Type", a.isSet ? "Encrypted set (one archive per file)" : "Archive"});
        rows.add(new String[]{"Folders", String.valueOf(folders.size())});
        if (a.file.lastModified() > 0) rows.add(new String[]{"Modified", Fmt.date(a.file.lastModified() / 1000)});
        if (!a.isSet && a.file.isFile()) rows.add(new String[]{"Size on disk", Fmt.size(a.file.length())});
        rows.add(new String[]{"Location", a.file.getPath(), "stack"});
        sh.space(16).content(detailCard(rows)).space(4).button("OK", Ui.FILLED, null).show();
    }

    private LinearLayout.LayoutParams statLp(boolean middle) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        if (middle) lp.setMargins(dp(8), 0, dp(8), 0);
        return lp;
    }

    // ------------------------------------------------------------------ pack

    private void packDialog(final List<File> sources) {
        if (sources.isEmpty()) return;
        String base = sources.size() == 1 ? Fmt.stripExt(sources.get(0).getName())
                : (isStorageRoot(cwd) ? "archive" : cwd.getName());
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
        final CheckBox wipe = Ui.check(this, "Delete original files after packing",
                Defaults.deleteAfterPack(this));
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
                .content(store).content(each).content(wipe).content(note)
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
                    final boolean wipeSrc = wipe.isChecked();
                    if (dest.exists()) {
                        confirm("Replace \"" + n + "\"?", "It already exists.", "Replace", Ui.DANGER,
                                () -> runPack(sources, dest, encrypt, separate, noCompress, true, pw, wipeSrc));
                    } else {
                        runPack(sources, dest, encrypt, separate, noCompress, false, pw, wipeSrc);
                    }
                    return true;
                }).show();
    }

    private void runPack(final List<File> sources, final File dest, final boolean encrypt,
                         final boolean separate, final boolean noCompress,
                         final boolean overwrite, final String pw, final boolean wipeSrc) {
        if (Priv.involves(pathsJson(sources), dest.getPath())) {
            message("Can't pack here yet", "Packing from or into protected folders isn't supported yet. "
                    + "Copy the files to Internal storage first, or extract into the protected folder instead.");
            return;
        }
        runOp("Packing...", true, p -> call("pack", pathsJson(sources), dest.getPath(), encrypt,
                separate, noCompress, overwrite, pw, wipeSrc, p), r -> {
            if (r.optBoolean("ok")) {
                sel.clear();
                loadDir(cwd);
                packResult(dest, r.optString("message"), encrypt, separate, noCompress,
                        wipeSrc ? r.optInt("deleted", 0) : -1, r.optInt("delete_failed", 0));
            } else if (r.optBoolean("cancelled")) {
                toast("Cancelled");
            } else {
                message("Couldn't create the archive", r.optString("message"));
            }
        });
    }

    /** The result of Pack: a header, three number tiles, how much space it saved, and the details. */
    private void packResult(File dest, String msg, boolean encrypt, boolean separate, boolean noCompress,
                            int deleted, int deleteFailed) {
        int tone = Ui.accent(this);
        LinearLayout head = new LinearLayout(this);
        head.setGravity(Gravity.CENTER_VERTICAL);
        ImageView ic = new ImageView(this);
        ic.setImageResource(separate ? R.drawable.ic_set : encrypt ? R.drawable.ic_lock : R.drawable.ic_archive);
        Ui.tint(ic, tone);
        ic.setPadding(dp(13), dp(13), dp(13), dp(13));
        ic.setBackground(Ui.shape(this, Ui.alpha(tone, 0x26), 0, 16));
        head.addView(ic, new LinearLayout.LayoutParams(dp(54), dp(54)));
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(this, separate ? "Archives created" : "Archive created", 18, true, false));
        TextView nm = Ui.label(this, dest.getName(), 13, false, true);
        nm.setSingleLine(true);
        nm.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        col.addView(nm);
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        cp.leftMargin = dp(14);
        head.addView(col, cp);
        Sheet sh = new Sheet(this, null).content(head).space(16);

        List<String[]> rows = new ArrayList<>();
        // kiln's line: "3 file(s): 40.0 MB -> 40.0 MB (100.0% of original) in 3.6s [1 core]"
        //          or: "18 archive(s) in /path: 65.4 KB -> 16.2 KB (24.8% of original) in 1.6s"
        java.util.regex.Matcher m = java.util.regex.Pattern.compile(
                "(\\d+) (file|archive)\\(s\\).*?: (.+?) -> (.+?) \\(([\\d.]+)% of original\\) in ([\\d.]+)s")
                .matcher(msg);
        if (m.find()) {
            int n = Integer.parseInt(m.group(1));
            boolean arcs = m.group(2).equals("archive");
            String orig = m.group(3), packed = m.group(4), time = m.group(6);
            double pct = Double.parseDouble(m.group(5));
            LinearLayout stats = new LinearLayout(this);
            stats.addView(statTile(String.valueOf(n), arcs ? (n == 1 ? "archive" : "archives")
                    : (n == 1 ? "file packed" : "files packed")), statLp(false));
            stats.addView(statTile(packed, "stored"), statLp(true));
            stats.addView(statTile(time + " s", "took"), statLp(false));
            sh.content(stats).space(16);

            LinearLayout cmp = new LinearLayout(this);
            cmp.setOrientation(LinearLayout.VERTICAL);
            cmp.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 14));
            cmp.setPadding(dp(14), dp(12), dp(14), dp(12));
            LinearLayout top = new LinearLayout(this);
            top.addView(Ui.label(this, "Compression", 13, false, true),
                    new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
            TextView right = Ui.label(this, pct >= 100 ? "No saving"
                    : "Saved " + Math.round(100 - pct) + "%", 13, true, false);
            right.setTextColor(tone);
            top.addView(right);
            cmp.addView(top);
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, dp(8));
            bp.topMargin = dp(8);
            cmp.addView(Ui.meter(this, pct / 100.0, true), bp);
            TextView cap = Ui.label(this, orig + " became " + packed, 12, false, true);
            LinearLayout.LayoutParams capP = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            capP.topMargin = dp(6);
            cmp.addView(cap, capP);
            sh.content(cmp).space(14);
        } else {
            rows.add(new String[]{"Result", msg.isEmpty() ? "Done" : msg, "stack"});
        }
        rows.add(new String[]{"Location", dest.getPath(), "stack"});
        rows.add(new String[]{"Layout", separate ? (encrypt ? "One archive per file, random names"
            : "One archive per file") : "Single archive"});
        rows.add(new String[]{"Encryption", encrypt ? "AES-256-GCM" : "None"});
        if (noCompress) rows.add(new String[]{"Compression", "Stored only"});
        if (deleted >= 0) {                              // -1: the option was off
            rows.add(new String[]{"Originals", plural(deleted, "file deleted", "files deleted")
                    + (deleteFailed > 0 ? ", " + deleteFailed + " couldn't be deleted" : "")});
        }
        sh.content(detailCard(rows)).space(4).button("OK", Ui.FILLED, null).show();
    }

    /** One line of the folder chooser. */
    private static final class Pick {
        static final int NEW = 0, UP = 1, DIR = 2, FILE = 3, VOL = 4;
        final int type;
        final File f;
        final String label;
        Pick(int type, File f) { this(type, f, null); }
        Pick(int type, File f, String label) { this.type = type; this.f = f; this.label = label; }
    }

    /** Folder chooser: tap a folder to enter it, then "Select this folder". Files show dimmed. */
    private void pickFolder(File start, final OnFile cb) {
        if (!hasStorage()) {
            askStorage();
            return;
        }
        File s = start;
        while (s != null && !s.isDirectory()) s = s.getParentFile();
        final File[] cur = {s != null ? s : root};
        final List<Pick> entries = new ArrayList<>();
        final TextView path = Ui.label(this, "", 13, false, true);
        path.setEllipsize(TextUtils.TruncateAt.START);
        path.setSingleLine(true);
        final ListView lv = new ListView(this);
        lv.setDivider(null);
        lv.setSelector(android.R.color.transparent);
        final BaseAdapter ad = new BaseAdapter() {
            @Override public int getCount() { return entries.size(); }
            @Override public Object getItem(int i) { return entries.get(i); }
            @Override public long getItemId(int i) { return i; }

            @Override
            public View getView(int i, View convert, ViewGroup parent) {
                LinearLayout row;
                ImageView icon;
                TextView name;
                if (convert instanceof LinearLayout) {
                    row = (LinearLayout) convert;
                    icon = (ImageView) row.getChildAt(0);
                    name = (TextView) row.getChildAt(1);
                } else {
                    row = new LinearLayout(MainActivity.this);
                    row.setGravity(Gravity.CENTER_VERTICAL);
                    row.setPadding(dp(4), dp(6), dp(4), dp(6));
                    icon = Thumbs.box(MainActivity.this);
                    row.addView(icon, new LinearLayout.LayoutParams(dp(40), dp(40)));
                    name = Ui.label(MainActivity.this, "", 16, false, false);
                    name.setSingleLine(true);
                    name.setEllipsize(TextUtils.TruncateAt.MIDDLE);
                    LinearLayout.LayoutParams np = new LinearLayout.LayoutParams(0,
                            ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
                    np.leftMargin = dp(14);
                    row.addView(name, np);
                }
                Pick e = entries.get(i);
                int accent = Ui.accent(MainActivity.this);
                row.setBackground(e.type == Pick.FILE ? null : Ui.pressable(MainActivity.this, 0, 0, 12));
                row.setAlpha(e.type == Pick.FILE ? 0.5f : 1f);
                icon.setTag(null);
                switch (e.type) {
                    case Pick.NEW:
                        Thumbs.showIcon(icon, R.drawable.ic_new_folder, accent);
                        name.setText("New folder...");
                        break;
                    case Pick.UP:
                        Thumbs.showIcon(icon, R.drawable.ic_up_dir, accent);
                        name.setText("..");
                        break;
                    case Pick.DIR:
                        Thumbs.showIcon(icon, dirIcon(e.f), accent);
                        name.setText(e.f.getName());
                        break;
                    case Pick.VOL:
                        Thumbs.showIcon(icon, e.f.equals(root) ? R.drawable.ic_home : R.drawable.ic_usb, accent);
                        name.setText(e.label);
                        break;
                    default:
                        Thumbs.bind(icon, e.f, Thumbs.iconFor(e.f.getName()),
                                Icons.tintFor(MainActivity.this, e.f.getName()), previewsOn);
                        name.setText(e.f.getName());
                        break;
                }
                return row;
            }
        };
        lv.setAdapter(ad);
        final Runnable[] fill = new Runnable[1];
        fill[0] = () -> {
            entries.clear();
            entries.add(new Pick(Pick.NEW, null));
            File parent = cur[0].getParentFile();
            if (parent != null && !isStorageRoot(cur[0])) entries.add(new Pick(Pick.UP, parent));
            if (isStorageRoot(cur[0])) {                       // the other storages sit beside this one
                if (!cur[0].equals(root)) entries.add(new Pick(Pick.VOL, root, "Internal storage"));
                for (File[] v : volumes()) {
                    if (!v[0].equals(cur[0])) entries.add(new Pick(Pick.VOL, v[0], v[1].getName()));
                }
            }
            File[] kids = Priv.wants(cur[0].getPath()) ? Priv.kids(cur[0]) : cur[0].listFiles();
            List<File> dirs = new ArrayList<>(), files = new ArrayList<>();
            if (kids != null) {
                for (File k : kids) {
                    if (k.getName().startsWith(".")) continue;
                    (k.isDirectory() ? dirs : files).add(k);
                }
            }
            java.util.Comparator<File> byName = (x, y) -> x.getName().compareToIgnoreCase(y.getName());
            Collections.sort(dirs, byName);
            Collections.sort(files, byName);
            for (File k : dirs) entries.add(new Pick(Pick.DIR, k));
            for (File k : files) entries.add(new Pick(Pick.FILE, k));
            path.setText(cur[0].getPath());
            lv.scheduleLayoutAnimation();
            ad.notifyDataSetChanged();
        };
        lv.setLayoutAnimation(Ui.rowAnim(this));
        lv.setOnItemClickListener((av, v, pos, id) -> {
            Pick e = entries.get(pos);
            if (e.type == Pick.FILE) return;
            if (e.type == Pick.NEW) {
                nameSheet("New folder", "", "Create", n -> {
                    File nf = new File(cur[0], n);
                    if (Priv.wants(nf.getPath())) {
                        String er = Priv.mkdirs(nf.getPath());
                        if (er != null) return er;
                        cur[0] = new Priv.PFile(nf.getPath(), true);
                        fill[0].run();
                        return null;
                    }
                    if (nf.exists()) return "That name is taken";
                    if (!nf.mkdirs()) return "Couldn't create it";
                    cur[0] = nf;
                    fill[0].run();
                    return null;
                });
                return;
            }
            cur[0] = e.f;
            fill[0].run();
        });
        fill[0].run();
        new Sheet(this, "Choose a folder").content(path).space(6).fixed(lv, 320)
                .button("Cancel", Ui.TEXT, null)
                .button("Select this folder", Ui.FILLED, () -> {
                    cb.got(cur[0]);
                    return true;
                }).show();
    }

    private void aboutDialog() {
        final String ver = getVersionName();
        long code = 0;
        try {
            code = getPackageManager().getPackageInfo(getPackageName(), 0).getLongVersionCode();
        } catch (Exception ignored) {
        }
        final long build = code;
        // the engine and script details come from Python: ask off the UI thread, then show
        bg(p -> {
            JSONObject o = new JSONObject();
            if (app != null) {
                try {
                    JSONObject v = call("version");
                    o.put("engine", v.optString("kiln"));
                    o.put("helper", v.optString("helper"));
                } catch (Exception ignored) {
                }
                try {
                    PyObject m = Python.getInstance().getModule("kiln_scripts");
                    JSONObject sc = new JSONObject(m.callAttr("status", getFilesDir().getPath()).toString());
                    o.put("main", sc.optString("main"));
                    o.put("main_version", sc.optString("main_version"));
                } catch (Exception ignored) {
                }
            }
            return o;
        }, o -> showAbout(ver, build, o));
    }

    private LinearLayout detailCard(List<String[]> rows) { return Ui.detailCard(this, rows); }

    private View statTile(String big, String small) { return Ui.statTile(this, big, small); }

    private void showAbout(String ver, long build, JSONObject o) {
        // header: the mark in a tile, the name, and the version as a small chip
        LinearLayout head = new LinearLayout(this);
        head.setOrientation(LinearLayout.VERTICAL);
        head.setGravity(Gravity.CENTER_HORIZONTAL);
        ImageView mark = new ImageView(this);
        mark.setImageResource(R.drawable.ic_kiln);
        mark.setScaleType(ImageView.ScaleType.FIT_CENTER);
        mark.setPadding(dp(14), dp(14), dp(14), dp(14));
        mark.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 20));
        head.addView(mark, new LinearLayout.LayoutParams(dp(72), dp(72)));
        TextView name = Ui.label(this, "Kiln", 24, true, false);
        name.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams np = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        np.topMargin = dp(12);
        head.addView(name, np);
        TextView chip = Ui.label(this, "Version " + ver, 13, true, false);
        chip.setTextColor(Ui.accent(this));
        chip.setPadding(dp(12), dp(4), dp(12), dp(4));
        chip.setBackground(Ui.shape(this, getColor(R.color.k_chip), 0, 12));
        LinearLayout.LayoutParams cp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        cp.topMargin = dp(6);
        head.addView(chip, cp);
        TextView tag = Ui.label(this, "Open, create and unpack Kiln archives, and manage ordinary files.",
                14, false, true);
        tag.setGravity(Gravity.CENTER);
        tag.setLineSpacing(0, 1.15f);
        LinearLayout.LayoutParams tp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        tp.topMargin = dp(10);
        tp.bottomMargin = dp(16);
        head.addView(tag, tp);

        // details: label on the left, value on the right, thin lines between
        String engine = o.optString("engine");
        String helper = o.optString("helper");
        String mainName = o.optString("main");
        List<String[]> rows = new ArrayList<>();
        rows.add(new String[]{"Version", ver + (build > 0 ? " (build " + build + ")" : "")});
        if (!engine.isEmpty()) rows.add(new String[]{"Engine", "kiln " + engine});
        if (app != null) rows.add(new String[]{"Fast mode", helper.isEmpty() ? "On" : "Off \u2013 " + helper});
        rows.add(new String[]{"Main script", mainName.isEmpty() ? "Built-in"
                : mainName + (o.optString("main_version").isEmpty() ? "" : " (v" + o.optString("main_version") + ")")});
        rows.add(new String[]{"Encryption", "AES-256-GCM"});

        LinearLayout card = detailCard(rows);
        new Sheet(this, null).content(head).content(card).space(4)
                .button("OK", Ui.FILLED, null).show();
    }

    private String getVersionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (Exception e) {
            return "";
        }
    }
}
