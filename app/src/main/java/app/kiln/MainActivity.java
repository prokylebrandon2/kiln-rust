package app.kiln;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.text.InputType;
import android.text.TextUtils;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
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
import android.widget.PopupMenu;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import com.chaquo.python.android.AndroidPlatform;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;

/** A file manager for Kiln archives: browse folders, open archives, extract, pack. */
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

    static final class Item { String name, kind; long size, mtime; boolean enc; }
    static final class Ent { String p; long s, m; }
    static final class Row {
        String key, title, sub;
        int icon;
        boolean dir;
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

    private static final int M_SHOW_ALL = 1, M_NEW_FOLDER = 2, M_REFRESH = 3, M_ABOUT = 4,
            M_SELECT_ALL = 5, M_EXTRACT = 6, M_RENAME = 7, M_DELETE = 8, M_CLEAR = 9,
            M_TEST = 10, M_INFO = 11, M_DECRYPT = 12, M_EXTRACT_ALL = 13;
    private static final long TEXT_PREVIEW_MAX = 1_000_000L;
    private static final long IMAGE_PREVIEW_MAX = 15_000_000L;
    private static final String TEXT_EXT = " txt md json xml csv log py c h cpp java kt sh html css js "
            + "yml yaml ini conf cfg toml tsv rst srt gradle properties ";
    private static final String IMAGE_EXT = " png jpg jpeg gif webp bmp ";

    // ------------------------------------------------------------------ state

    private final Handler ui = new Handler(Looper.getMainLooper());
    private final File root = Environment.getExternalStorageDirectory();
    private final List<Row> rows = new ArrayList<>();
    private final List<Item> items = new ArrayList<>();
    private final LinkedHashSet<String> sel = new LinkedHashSet<>();
    private volatile PyObject app;           // kiln_app module; null until Python is up
    private volatile boolean svcOn = false;
    private boolean showAll = false;
    private boolean loadedOnce = false;
    private boolean askedNotif = false;
    private File cwd = Environment.getExternalStorageDirectory();
    private Arc arc;                         // non-null while an archive is open
    private ProgressUi currentProgress;

    private ImageButton left, more;
    private TextView title, sub, empty;
    private Button primary;
    private ListView list;
    private View permView;
    private RowAdapter adapter;

    // ------------------------------------------------------------------ lifecycle

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        deleteTree(new File(getCacheDir(), "preview"));
        buildUi();
        startPython();
    }

    @Override
    protected void onResume() {
        super.onResume();
        refreshPermission();
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

    // ------------------------------------------------------------------ small UI helpers

    private int dp(int v) { return Math.round(v * getResources().getDisplayMetrics().density); }

    private int accent() {
        TypedValue tv = new TypedValue();
        getTheme().resolveAttribute(android.R.attr.colorAccent, tv, true);
        return tv.data;
    }

    private Drawable ripple() {
        TypedValue tv = new TypedValue();
        getTheme().resolveAttribute(android.R.attr.selectableItemBackgroundBorderless, tv, true);
        return getDrawable(tv.resourceId);
    }

    private TextView text(String s, int sp, boolean bold) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(sp);
        if (bold) t.setTypeface(Typeface.DEFAULT_BOLD);
        return t;
    }

    private ImageButton iconButton(int res, View.OnClickListener l) {
        ImageButton b = new ImageButton(this);
        b.setImageResource(res);
        b.setBackground(ripple());
        b.setOnClickListener(l);
        return b;
    }

    private EditText edit(String hint, boolean secret) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setSingleLine(true);
        e.setInputType(secret
                ? (InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD)
                : InputType.TYPE_CLASS_TEXT);
        return e;
    }

    private CheckBox check(String label, boolean on) {
        CheckBox c = new CheckBox(this);
        c.setText(label);
        c.setChecked(on);
        return c;
    }

    private Button button(String label, View.OnClickListener l) {
        Button b = new Button(this);
        b.setText(label);
        b.setAllCaps(false);
        b.setOnClickListener(l);
        return b;
    }

    private LinearLayout form() {
        LinearLayout f = new LinearLayout(this);
        f.setOrientation(LinearLayout.VERTICAL);
        f.setPadding(dp(24), dp(8), dp(24), dp(0));
        return f;
    }

    private LinearLayout rowOf(View grow, View fixed) {
        LinearLayout r = new LinearLayout(this);
        r.setOrientation(LinearLayout.HORIZONTAL);
        r.setGravity(Gravity.CENTER_VERTICAL);
        r.addView(grow, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        r.addView(fixed, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        return r;
    }

    private void toast(String s) { Toast.makeText(this, s, Toast.LENGTH_SHORT).show(); }

    private void message(String heading, String body) {
        new AlertDialog.Builder(this).setTitle(heading).setMessage(body)
                .setPositiveButton("OK", null).show();
    }

    private void confirm(String body, String yes, final Runnable action) {
        new AlertDialog.Builder(this).setMessage(body).setNegativeButton("Cancel", null)
                .setPositiveButton(yes, (d, w) -> action.run()).show();
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

    private static boolean deleteTree(File f) {
        if (f.isDirectory()) {
            File[] kids = f.listFiles();
            if (kids != null) {
                for (File k : kids) deleteTree(k);
            }
        }
        return f.delete();
    }

    // ------------------------------------------------------------------ layout

    private void buildUi() {
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);

        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(dp(4), dp(6), dp(4), dp(6));
        left = iconButton(R.drawable.ic_up, v -> onLeft());
        bar.addView(left, new LinearLayout.LayoutParams(dp(48), dp(48)));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        title = text("Kiln", 19, true);
        title.setSingleLine(true);
        title.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        sub = text("", 12, false);
        sub.setSingleLine(true);
        sub.setEllipsize(TextUtils.TruncateAt.START);
        titles.addView(title);
        titles.addView(sub);
        bar.addView(titles, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        primary = button("", v -> onPrimary());
        bar.addView(primary);
        more = iconButton(R.drawable.ic_more, v -> showMenu());
        bar.addView(more, new LinearLayout.LayoutParams(dp(48), dp(48)));
        page.addView(bar);

        View line = new View(this);
        line.setBackgroundColor(Color.argb(40, 128, 128, 128));
        page.addView(line, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(1)));

        FrameLayout body = new FrameLayout(this);
        list = new ListView(this);
        list.setDivider(null);
        adapter = new RowAdapter();
        list.setAdapter(adapter);
        list.setOnItemClickListener((av, v, pos, id) -> onRowClick(pos));
        list.setOnItemLongClickListener((av, v, pos, id) -> {
            toggle(rows.get(pos).key);
            return true;
        });
        body.addView(list, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        empty = text("Starting...", 15, false);
        empty.setGravity(Gravity.CENTER);
        empty.setPadding(dp(32), dp(32), dp(32), dp(32));
        body.addView(empty, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        list.setEmptyView(empty);

        LinearLayout perm = new LinearLayout(this);
        perm.setOrientation(LinearLayout.VERTICAL);
        perm.setGravity(Gravity.CENTER);
        perm.setPadding(dp(32), dp(32), dp(32), dp(32));
        TextView why = text("Kiln needs access to your files to open, create and unpack archives. "
                + "Nothing leaves your phone.", 15, false);
        why.setGravity(Gravity.CENTER);
        perm.addView(why);
        Button grant = button("Allow access to files", v -> askStorage());
        LinearLayout.LayoutParams gp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        gp.topMargin = dp(16);
        perm.addView(grant, gp);
        permView = perm;
        permView.setVisibility(View.GONE);
        // opaque so it covers the list behind it
        TypedValue bg = new TypedValue();
        getTheme().resolveAttribute(android.R.attr.colorBackground, bg, true);
        permView.setBackgroundColor(bg.data);
        body.addView(permView, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        page.addView(body, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        setContentView(page);
        renderHeader();
    }

    /** One row of the list: icon, name, details, and a check mark when selected. */
    private final class RowView extends LinearLayout {
        final ImageView icon, check;
        final TextView name, detail;

        RowView() {
            super(MainActivity.this);
            setOrientation(HORIZONTAL);
            setGravity(Gravity.CENTER_VERTICAL);
            setPadding(dp(16), dp(10), dp(16), dp(10));
            icon = new ImageView(MainActivity.this);
            addView(icon, new LayoutParams(dp(32), dp(32)));
            LinearLayout col = new LinearLayout(MainActivity.this);
            col.setOrientation(VERTICAL);
            name = text("", 16, false);
            name.setSingleLine(true);
            name.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            detail = text("", 12, false);
            detail.setSingleLine(true);
            detail.setAlpha(0.7f);
            col.addView(name);
            col.addView(detail);
            LayoutParams cp = new LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            cp.leftMargin = dp(16);
            addView(col, cp);
            check = new ImageView(MainActivity.this);
            check.setImageResource(R.drawable.ic_check);
            addView(check, new LayoutParams(dp(24), dp(24)));
        }

        void bind(Row r, boolean selected) {
            icon.setImageResource(r.icon);
            name.setText(r.title);
            detail.setText(r.sub);
            check.setVisibility(selected ? View.VISIBLE : View.INVISIBLE);
            int a = accent();
            setBackgroundColor(selected
                    ? Color.argb(50, Color.red(a), Color.green(a), Color.blue(a)) : Color.TRANSPARENT);
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

    // ------------------------------------------------------------------ header and menu

    private void redraw() {
        adapter.notifyDataSetChanged();
        renderHeader();
    }

    private void renderHeader() {
        boolean selecting = !sel.isEmpty();
        left.setVisibility(View.VISIBLE);
        primary.setVisibility(View.GONE);
        if (arc != null) {
            if (selecting) {
                left.setImageResource(R.drawable.ic_close);
                title.setText(sel.size() + " selected");
                sub.setText("");
                primary.setText("Extract");
                primary.setVisibility(View.VISIBLE);
            } else {
                left.setImageResource(R.drawable.ic_up);
                title.setText(arc.file.getName());
                sub.setText(arc.prefix.isEmpty()
                        ? arc.ents.size() + (arc.ents.size() == 1 ? " file" : " files")
                        + (arc.encrypted ? " · encrypted" : "")
                        : "/" + arc.prefix);
                primary.setText("Extract all");
                primary.setVisibility(View.VISIBLE);
            }
        } else if (selecting) {
            left.setImageResource(R.drawable.ic_close);
            title.setText(sel.size() + " selected");
            sub.setText("");
            primary.setText("Pack");
            primary.setVisibility(View.VISIBLE);
        } else {
            left.setImageResource(R.drawable.ic_up);
            boolean atRoot = cwd.equals(root);
            left.setVisibility(atRoot ? View.INVISIBLE : View.VISIBLE);
            title.setText(atRoot ? "Kiln" : cwd.getName());
            sub.setText(cwd.getPath());
        }
    }

    private void onLeft() {
        if (!sel.isEmpty()) {
            sel.clear();
            redraw();
        } else {
            onBackPressed();
        }
    }

    private void onPrimary() {
        if (arc != null) {
            extractDialog(sel.isEmpty() ? new ArrayList<String>() : new ArrayList<>(sel));
        } else {
            packDialog(selectedFiles());
        }
    }

    private void showMenu() {
        PopupMenu m = new PopupMenu(this, more);
        boolean selecting = !sel.isEmpty();
        if (arc != null) {
            if (selecting) {
                m.getMenu().add(0, M_SELECT_ALL, 0, "Select all");
                m.getMenu().add(0, M_CLEAR, 1, "Clear selection");
            } else {
                m.getMenu().add(0, M_EXTRACT_ALL, 0, "Extract all...");
                m.getMenu().add(0, M_TEST, 1, "Check integrity");
                m.getMenu().add(0, M_SELECT_ALL, 2, "Select all");
                m.getMenu().add(0, M_INFO, 3, "Archive info");
                if (arc.isSet) m.getMenu().add(0, M_DECRYPT, 4, "Decrypt folder in place...");
            }
        } else if (selecting) {
            m.getMenu().add(0, M_SELECT_ALL, 0, "Select all");
            if (sel.size() == 1 && selectedArchive() != null) m.getMenu().add(0, M_EXTRACT, 1, "Extract...");
            if (sel.size() == 1) m.getMenu().add(0, M_RENAME, 2, "Rename");
            m.getMenu().add(0, M_DELETE, 3, "Delete");
            m.getMenu().add(0, M_CLEAR, 4, "Clear selection");
        } else {
            m.getMenu().add(0, M_SHOW_ALL, 0, showAll ? "Show archives only" : "Show all files");
            m.getMenu().add(0, M_NEW_FOLDER, 1, "New folder");
            m.getMenu().add(0, M_REFRESH, 2, "Refresh");
            m.getMenu().add(0, M_ABOUT, 3, "About");
        }
        m.setOnMenuItemClickListener(it -> {
            onMenu(it.getItemId());
            return true;
        });
        m.show();
    }

    private void onMenu(int id) {
        switch (id) {
            case M_SHOW_ALL: showAll = !showAll; loadDir(cwd); break;
            case M_NEW_FOLDER: newFolderDialog(cwd, f -> loadDir(cwd)); break;
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
            case M_RENAME: renameDialog(); break;
            case M_DELETE: deleteSelected(); break;
            case M_TEST: runTest(); break;
            case M_INFO: infoDialog(); break;
            case M_DECRYPT: decryptInPlace(); break;
            case M_EXTRACT_ALL: extractDialog(new ArrayList<String>()); break;
            default: break;
        }
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
                });
            } catch (Throwable t) {
                ui.post(() -> {
                    empty.setText("Kiln couldn't start: " + t);
                });
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
    }

    // ------------------------------------------------------------------ browsing

    private void loadDir(File dir) {
        cwd = dir;
        arc = null;
        sel.clear();
        rows.clear();
        items.clear();
        empty.setText("Loading...");
        redraw();
        final String path = dir.getPath();
        final boolean all = showAll;
        new Thread(() -> {
            JSONObject r;
            try {
                r = call("scan", path, all);
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
            it.enc = o.optBoolean("e");
            items.add(it);
            Row row = new Row();
            row.key = it.name;
            row.title = it.name;
            row.item = it;
            row.dir = it.kind.equals("dir");
            String when = Fmt.date(it.mtime);
            switch (it.kind) {
                case "dir":
                    row.icon = R.drawable.ic_folder;
                    row.sub = when;
                    break;
                case "set":
                    row.icon = R.drawable.ic_set;
                    row.sub = "Encrypted set · " + when;
                    break;
                case "archive":
                    row.icon = it.enc ? R.drawable.ic_lock : R.drawable.ic_archive;
                    row.sub = Fmt.size(it.size) + (it.enc ? " · encrypted" : "") + " · " + when;
                    break;
                case "old":
                    row.icon = R.drawable.ic_archive;
                    row.sub = "Older Kiln format · can't be opened here";
                    break;
                default:
                    row.icon = R.drawable.ic_file;
                    row.sub = Fmt.size(it.size) + " · " + when;
                    break;
            }
            rows.add(row);
        }
        empty.setText(showAll
                ? "This folder is empty."
                : "No Kiln archives here.\nTap the menu to show all files, then long-press files or "
                + "folders to pack them.");
        redraw();
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
                fileDialog(r);
            }
            return;
        }
        Item it = r.item;
        switch (it.kind) {
            case "dir": loadDir(new File(cwd, it.name)); break;
            case "archive":
            case "set": openItem(it, null); break;
            case "old":
                message("Older format", "This archive was made by an older Kiln (format 9 or earlier). "
                        + "Open it with kiln_v13 in Termux.");
                break;
            default:
                toast("Not a Kiln archive. Long-press to select it for packing.");
                break;
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
        final LinearLayout f = form();
        final EditText pw = edit("Password", true);
        final CheckBox show = check("Show password", false);
        show.setOnCheckedChangeListener((b, on) -> {
            pw.setInputType(InputType.TYPE_CLASS_TEXT | (on
                    ? InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                    : InputType.TYPE_TEXT_VARIATION_PASSWORD));
            pw.setSelection(pw.getText().length());
        });
        f.addView(pw);
        f.addView(show);
        final AlertDialog d = new AlertDialog.Builder(this).setTitle("Unlock " + name).setView(f)
                .setNegativeButton("Cancel", null).setPositiveButton("Unlock", null).create();
        d.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        d.show();
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            String s = pw.getText().toString();
            if (s.isEmpty()) {
                pw.setError("Enter the password");
                return;
            }
            d.dismiss();
            cb.got(s);
        });
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
                    agg = new long[2];
                    dirs.put(d, agg);
                }
                agg[0]++;
                agg[1] += Math.max(e.s, 0);
            }
        }
        List<String> names = new ArrayList<>(dirs.keySet());
        Collections.sort(names, String.CASE_INSENSITIVE_ORDER);
        for (String d : names) {
            long[] agg = dirs.get(d);
            Row r = new Row();
            r.key = prefix + d + "/";
            r.title = d;
            r.dir = true;
            r.icon = R.drawable.ic_folder;
            r.sub = agg[0] + (agg[0] == 1 ? " file" : " files") + " · " + Fmt.size(agg[1]);
            rows.add(r);
        }
        Collections.sort(files, (a, b) -> a.p.compareToIgnoreCase(b.p));
        for (Ent e : files) {
            Row r = new Row();
            r.key = e.p;
            r.title = e.p.substring(prefix.length());
            r.ent = e;
            r.icon = R.drawable.ic_file;
            r.sub = Fmt.size(e.s) + (e.m > 0 ? " · " + Fmt.date(e.m) : "");
            rows.add(r);
        }
        empty.setText("This folder is empty.");
        redraw();
    }

    // ------------------------------------------------------------------ progress and jobs

    /** Modal progress box. cancelP == null hides the Cancel button. */
    private final class ProgressUi {
        final AlertDialog dialog;
        final ProgressBar bar;
        final TextView line;

        ProgressUi(String heading, final Progress cancelP) {
            LinearLayout f = form();
            line = text("", 13, false);
            line.setSingleLine(true);
            line.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            bar = new ProgressBar(MainActivity.this, null, android.R.attr.progressBarStyleHorizontal);
            bar.setIndeterminate(true);
            bar.setMax(1000);
            f.addView(line);
            LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            bp.topMargin = dp(12);
            f.addView(bar, bp);
            AlertDialog.Builder b = new AlertDialog.Builder(MainActivity.this)
                    .setTitle(heading).setView(f).setCancelable(false);
            if (cancelP != null) b.setNegativeButton("Cancel", null);
            dialog = b.create();
            dialog.show();
            if (cancelP != null) {
                dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setOnClickListener(v -> {
                    cancelP.cancel();
                    line.setText("Cancelling...");
                    v.setEnabled(false);
                });
            }
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

        void close() {
            try {
                dialog.dismiss();
            } catch (Throwable ignored) {
            }
        }
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

    private void fileDialog(final Row r) {
        final String name = r.title;
        final String ext = Fmt.ext(name);
        boolean text = TEXT_EXT.contains(" " + ext + " ") && r.ent.s <= TEXT_PREVIEW_MAX;
        boolean image = IMAGE_EXT.contains(" " + ext + " ") && r.ent.s <= IMAGE_PREVIEW_MAX;
        final List<String> labels = new ArrayList<>();
        if (text || image) labels.add("Preview");
        labels.add("Extract...");
        new AlertDialog.Builder(this).setTitle(name)
                .setItems(labels.toArray(new String[0]), (d, which) -> {
                    if (labels.get(which).equals("Preview")) {
                        previewFile(r.ent.p);
                    } else {
                        List<String> k = new ArrayList<>();
                        k.add(r.key);
                        extractDialog(k);
                    }
                }).show();
    }

    private void previewFile(final String member) {
        final Arc a = arc;
        final File cache = new File(getCacheDir(), "preview");
        cache.mkdirs();
        runOp("Preparing preview...", false,
                p -> call("preview", a.file.getPath(), a.password, member, cache.getPath()),
                r -> {
                    if (!r.optBoolean("ok")) {
                        message("Couldn't preview", r.optString("message"));
                        return;
                    }
                    showPreview(new File(r.optString("file")), member);
                });
    }

    private void showPreview(final File f, String member) {
        View content;
        String ext = Fmt.ext(f.getName());
        if (IMAGE_EXT.contains(" " + ext + " ")) {
            BitmapFactory.Options o = new BitmapFactory.Options();
            o.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(f.getPath(), o);
            int sample = 1;
            while (o.outWidth / sample > 2048 || o.outHeight / sample > 2048) sample *= 2;
            o = new BitmapFactory.Options();
            o.inSampleSize = sample;
            Bitmap bmp = BitmapFactory.decodeFile(f.getPath(), o);
            if (bmp == null) {
                message("Couldn't preview", "This image can't be decoded.");
                deleteTree(f.getParentFile());
                return;
            }
            ImageView iv = new ImageView(this);
            iv.setImageBitmap(bmp);
            iv.setAdjustViewBounds(true);
            iv.setPadding(dp(8), dp(8), dp(8), dp(8));
            content = iv;
        } else {
            String body;
            try (FileInputStream in = new FileInputStream(f)) {
                byte[] buf = new byte[(int) Math.min(f.length(), TEXT_PREVIEW_MAX)];
                int n = 0;
                while (n < buf.length) {
                    int got = in.read(buf, n, buf.length - n);
                    if (got < 0) break;
                    n += got;
                }
                body = new String(buf, 0, n, java.nio.charset.StandardCharsets.UTF_8);
            } catch (IOException e) {
                body = "Couldn't read the file: " + e.getMessage();
            }
            TextView t = text(body, 12, false);
            t.setTypeface(Typeface.MONOSPACE);
            t.setTextIsSelectable(true);
            t.setPadding(dp(16), dp(8), dp(16), dp(8));
            ScrollView sv = new ScrollView(this);
            sv.addView(t);
            content = sv;
        }
        new AlertDialog.Builder(this).setTitle(member.substring(member.lastIndexOf('/') + 1))
                .setView(content).setPositiveButton("Close", null)
                .setOnDismissListener(d -> deleteTree(f.getParentFile())).show();
    }

    private void extractDialog(final List<String> keys) {
        final Arc a = arc;
        File defDest = new File(a.file.getParentFile(),
                a.isSet ? a.file.getName() + "_files" : Fmt.stripExt(a.file.getName()));
        final LinearLayout f = form();
        f.addView(text(keys.isEmpty() ? "Extract all " + a.ents.size() + " files to:"
                : "Extract " + countFor(keys) + " file(s) to:", 14, false));
        final EditText where = edit("Folder", false);
        where.setText(defDest.getPath());
        f.addView(rowOf(where, button("Browse", v -> pickFolder(
                new File(where.getText().toString()), picked -> where.setText(picked.getPath())))));
        final CheckBox replace = check("Replace files that already exist", false);
        f.addView(replace);
        final AlertDialog d = new AlertDialog.Builder(this).setTitle("Extract").setView(f)
                .setNegativeButton("Cancel", null).setPositiveButton("Extract", null).create();
        d.show();
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            String dest = where.getText().toString().trim();
            if (dest.isEmpty()) {
                where.setError("Choose a folder");
                return;
            }
            d.dismiss();
            runExtract(keys, dest, replace.isChecked());
        });
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
                        new AlertDialog.Builder(this).setTitle("Done").setMessage(msg)
                                .setNegativeButton("OK", null)
                                .setPositiveButton("Open folder", (d, w) -> {
                                    arc = null;
                                    loadDir(new File(dest));
                                }).show();
                    } else if (r.optBoolean("cancelled")) {
                        toast("Cancelled");
                    } else if (!overwrite && msg.contains("already exist")) {
                        confirm(msg + "\n\nReplace them?", "Replace",
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
        confirm("This turns \"" + a.file.getName() + "\" into an ordinary folder: the encryption and "
                        + "the random file names are removed and the files become plain files.",
                "Decrypt", () -> runOp("Decrypting...", true,
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
        final LinearLayout f = form();
        String base = sources.size() == 1 ? Fmt.stripExt(sources.get(0).getName())
                : (cwd.equals(root) ? "archive" : cwd.getName());
        final EditText name = edit("Name", false);
        name.setText(base + ".kln");
        f.addView(text(sources.size() == 1 ? "Pack \"" + sources.get(0).getName() + "\""
                : "Pack " + sources.size() + " items", 14, true));
        f.addView(name);
        final EditText where = edit("Save in", false);
        where.setText(cwd.getPath());
        f.addView(rowOf(where, button("Browse", v -> pickFolder(
                new File(where.getText().toString()), picked -> where.setText(picked.getPath())))));
        final CheckBox enc = check("Encrypt with a password", true);
        final EditText p1 = edit("Password (8+ characters)", true);
        final EditText p2 = edit("Repeat password", true);
        final CheckBox show = check("Show passwords", false);
        final CheckBox store = check("Don't compress (faster for photos, music, video)", false);
        final CheckBox each = check("Separate archive for each file", false);
        final TextView note = text("", 12, false);
        note.setAlpha(0.7f);
        f.addView(enc);
        f.addView(p1);
        f.addView(p2);
        f.addView(show);
        f.addView(store);
        f.addView(each);
        f.addView(note);
        show.setOnCheckedChangeListener((b, on) -> {
            int t = InputType.TYPE_CLASS_TEXT | (on
                    ? InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                    : InputType.TYPE_TEXT_VARIATION_PASSWORD);
            p1.setInputType(t);
            p2.setInputType(t);
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
        ScrollView sv = new ScrollView(this);
        sv.addView(f);
        final AlertDialog d = new AlertDialog.Builder(this).setTitle("Create archive").setView(sv)
                .setNegativeButton("Cancel", null).setPositiveButton("Create", null).create();
        d.show();
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            String n = name.getText().toString().trim();
            if (n.isEmpty() || n.contains("/")) {
                name.setError("Enter a name (no slashes)");
                return;
            }
            File dir = new File(where.getText().toString().trim());
            if (!dir.isDirectory()) {
                where.setError("That folder doesn't exist");
                return;
            }
            final boolean encrypt = enc.isChecked();
            final String pw = p1.getText().toString();
            if (encrypt) {
                if (pw.length() < 8) {
                    p1.setError("At least 8 characters");
                    return;
                }
                if (!pw.equals(p2.getText().toString())) {
                    p2.setError("Passwords don't match");
                    return;
                }
            }
            final File dest = new File(dir, n);
            final boolean separate = each.isChecked();
            final boolean noCompress = store.isChecked();
            d.dismiss();
            if (dest.exists()) {
                confirm("\"" + n + "\" already exists. Replace it?", "Replace",
                        () -> runPack(sources, dest, encrypt, separate, noCompress, true, pw));
            } else {
                runPack(sources, dest, encrypt, separate, noCompress, false, pw);
            }
        });
    }

    private void runPack(final List<File> sources, final File dest, final boolean encrypt,
                         final boolean separate, final boolean noCompress,
                         final boolean overwrite, final String pw) {
        runOp("Packing...", true, p -> {
            JSONArray a = new JSONArray();
            for (File s : sources) a.put(s.getPath());
            return call("pack", a.toString(), dest.getPath(), encrypt, separate, noCompress,
                    overwrite, pw, p);
        }, r -> {
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

    // ------------------------------------------------------------------ file operations

    private void newFolderDialog(final File parent, final OnFile cb) {
        final EditText name = edit("Folder name", false);
        final LinearLayout f = form();
        f.addView(name);
        final AlertDialog d = new AlertDialog.Builder(this).setTitle("New folder").setView(f)
                .setNegativeButton("Cancel", null).setPositiveButton("Create", null).create();
        d.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        d.show();
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            String n = name.getText().toString().trim();
            if (n.isEmpty() || n.contains("/")) {
                name.setError("Enter a name (no slashes)");
                return;
            }
            File nf = new File(parent, n);
            if (nf.exists() || !nf.mkdirs()) {
                name.setError("Can't create it (already exists?)");
                return;
            }
            d.dismiss();
            cb.got(nf);
        });
        name.requestFocus();
    }

    private void renameDialog() {
        if (sel.size() != 1) return;
        final String old = sel.iterator().next();
        final EditText name = edit("New name", false);
        name.setText(old);
        final LinearLayout f = form();
        f.addView(name);
        final AlertDialog d = new AlertDialog.Builder(this).setTitle("Rename").setView(f)
                .setNegativeButton("Cancel", null).setPositiveButton("Rename", null).create();
        d.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        d.show();
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            String n = name.getText().toString().trim();
            if (n.isEmpty() || n.contains("/")) {
                name.setError("Enter a name (no slashes)");
                return;
            }
            File target = new File(cwd, n);
            if (target.exists()) {
                name.setError("That name is taken");
                return;
            }
            if (!new File(cwd, old).renameTo(target)) {
                name.setError("Couldn't rename");
                return;
            }
            d.dismiss();
            loadDir(cwd);
        });
        name.requestFocus();
    }

    private void deleteSelected() {
        final List<File> victims = selectedFiles();
        if (victims.isEmpty()) return;
        String what = victims.size() == 1 ? "\"" + victims.get(0).getName() + "\""
                : victims.size() + " items";
        confirm("Delete " + what + "? This can't be undone.", "Delete", () -> {
            new Thread(() -> {
                int failed = 0;
                for (File v : victims) {
                    if (!deleteTree(v)) failed++;
                }
                final int bad = failed;
                ui.post(() -> {
                    if (bad > 0) toast(bad + " item(s) couldn't be deleted");
                    loadDir(cwd);
                });
            }).start();
        });
    }

    /** Folder chooser: tap a folder to enter it, then "Select". */
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
        final AlertDialog d = new AlertDialog.Builder(this).setView(lv)
                .setNegativeButton("Cancel", null)
                .setNeutralButton("New folder", null)
                .setPositiveButton("Select this folder", null).create();
        final Runnable fill = () -> {
            dirs.clear();
            labels.clear();
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
            d.setTitle(cur[0].getPath());
            ad.notifyDataSetChanged();
        };
        d.show();
        lv.setOnItemClickListener((av, v, pos, id) -> {
            cur[0] = dirs.get(pos);
            fill.run();
        });
        d.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            d.dismiss();
            cb.got(cur[0]);
        });
        d.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v ->
                newFolderDialog(cur[0], nf -> {
                    cur[0] = nf;
                    fill.run();
                }));
        fill.run();
    }

    private void aboutDialog() {
        String body = "Kiln " + getVersionName() + "\nOpen, create and unpack Kiln archives "
                + "(AES-256-GCM encryption).";
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
