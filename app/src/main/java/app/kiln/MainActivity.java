package app.kiln;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.graphics.Typeface;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.text.InputType;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import com.chaquo.python.android.AndroidPlatform;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

/** One screen that drives kiln_v14.py (through kiln_bridge.py) the way the command line does. */
public class MainActivity extends Activity {

    /** Python calls line() for every line kiln prints. Public so Python can reach it. */
    public static class LineSink {
        private final MainActivity owner;
        LineSink(MainActivity owner) { this.owner = owner; }
        public void line(String text) { owner.append(text); }
    }

    private static final String SEP = "\u001f";

    private final Handler ui = new Handler(Looper.getMainLooper());
    private final StringBuilder logText = new StringBuilder();
    private boolean flushPending = false;
    private volatile boolean busy = false;
    private volatile PyObject bridge;          // kiln_bridge module, null until Python is up

    private EditText src, dst, pass, pass2;
    private CheckBox encrypt, each, store, force, inPlace;
    private TextView log, status;
    private ScrollView scroll;
    private final List<Button> buttons = new ArrayList<>();

    // ------------------------------------------------------------------ UI

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        buildUi();
        startPython();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (status != null) {
            status.setText(hasStorage() ? "" : "Storage access is off: tap any button to grant it.");
        }
    }

    private int dp(int v) {
        return Math.round(v * getResources().getDisplayMetrics().density);
    }

    private TextView label(String text) {
        TextView t = new TextView(this);
        t.setText(text);
        t.setTextSize(13);
        t.setPadding(0, dp(12), 0, dp(2));
        return t;
    }

    private EditText field(String hint, boolean secret) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setSingleLine(true);
        e.setTextSize(15);
        e.setInputType(secret
                ? (InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD)
                : (InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI));
        return e;
    }

    private CheckBox check(String text, boolean on) {
        CheckBox c = new CheckBox(this);
        c.setText(text);
        c.setChecked(on);
        return c;
    }

    private LinearLayout row(View first, View second) {
        LinearLayout r = new LinearLayout(this);
        r.setOrientation(LinearLayout.HORIZONTAL);
        r.addView(first, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        r.addView(second, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        return r;
    }

    private Button actionButton(String text, final String cmd) {
        Button b = new Button(this);
        b.setText(text);
        b.setAllCaps(false);
        b.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { act(cmd); }
        });
        buttons.add(b);
        return b;
    }

    private Button browseButton(final EditText target, final boolean foldersOnly) {
        Button b = new Button(this);
        b.setText("Browse");
        b.setAllCaps(false);
        b.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { browse(target, foldersOnly); }
        });
        return b;
    }

    private void buildUi() {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(dp(16), dp(16), dp(16), dp(24));

        TextView title = new TextView(this);
        title.setText("Kiln");
        title.setTextSize(24);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        col.addView(title);

        status = new TextView(this);
        status.setTextSize(12);
        col.addView(status);

        col.addView(label("Source (archive, folder or random-name set)"));
        src = field("/storage/emulated/0/...", false);
        col.addView(row(src, browseButton(src, false)));

        col.addView(label("Output (folder to unpack into, or archive/folder name for pack)"));
        dst = field("/storage/emulated/0/...", false);
        col.addView(row(dst, browseButton(dst, true)));

        col.addView(label("Password"));
        pass = field("password", true);
        col.addView(pass);
        final TextView repeatLabel = label("Repeat password (pack with encryption)");
        pass2 = field("repeat password", true);
        col.addView(repeatLabel);
        col.addView(pass2);

        col.addView(label("Pack options"));
        encrypt = check("Encrypt (AES-256-GCM)", true);
        each = check("One archive per file (with encryption: random names + encrypted index)", false);
        store = check("Encrypt only, no compression", false);
        col.addView(encrypt);
        col.addView(each);
        col.addView(store);
        encrypt.setOnCheckedChangeListener(new CompoundButton.OnCheckedChangeListener() {
            @Override public void onCheckedChanged(CompoundButton b, boolean on) {
                int vis = on ? View.VISIBLE : View.GONE;
                repeatLabel.setVisibility(vis);
                pass2.setVisibility(vis);
                store.setVisibility(vis);
            }
        });

        col.addView(label("Unpack options"));
        inPlace = check("Decrypt a random-name set into itself (deletes the .kln files)", false);
        force = check("Overwrite existing files", false);
        col.addView(inPlace);
        col.addView(force);

        LinearLayout buttonsRow = new LinearLayout(this);
        buttonsRow.setOrientation(LinearLayout.HORIZONTAL);
        String[][] defs = {{"List", "list"}, {"Test", "test"}, {"Unpack", "unpack"}, {"Pack", "pack"}};
        for (String[] d : defs) {
            buttonsRow.addView(actionButton(d[0], d[1]),
                    new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        }
        LinearLayout.LayoutParams bp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        bp.topMargin = dp(16);
        col.addView(buttonsRow, bp);

        log = new TextView(this);
        log.setTypeface(Typeface.MONOSPACE);
        log.setTextSize(12);
        log.setTextIsSelectable(true);
        log.setPadding(0, dp(12), 0, 0);
        col.addView(log);

        scroll = new ScrollView(this);
        scroll.addView(col);
        setContentView(scroll);
    }

    // ------------------------------------------------------------------ log

    void append(String text) {
        synchronized (logText) {
            logText.append(text).append('\n');
            if (logText.length() > 80000) {
                logText.delete(0, logText.length() - 50000);
            }
            if (flushPending) {
                return;
            }
            flushPending = true;
        }
        ui.postDelayed(new Runnable() {
            @Override public void run() {
                String snapshot;
                synchronized (logText) {
                    flushPending = false;
                    snapshot = logText.toString();
                }
                log.setText(snapshot);
                scroll.post(new Runnable() {
                    @Override public void run() { scroll.fullScroll(View.FOCUS_DOWN); }
                });
            }
        }, 120);
    }

    private void toast(String s) {
        Toast.makeText(this, s, Toast.LENGTH_SHORT).show();
    }

    private void setBusy(boolean b) {
        busy = b;
        for (Button x : buttons) {
            x.setEnabled(!b);
        }
    }

    // ------------------------------------------------------------------ Python

    private void startPython() {
        append("Starting Python...");
        new Thread(new Runnable() {
            @Override public void run() {
                try {
                    if (!Python.isStarted()) {
                        Python.start(new AndroidPlatform(MainActivity.this));
                    }
                    Python py = Python.getInstance();
                    String helper = getApplicationInfo().nativeLibraryDir + "/libkiln_accel.so";
                    String note = py.getModule("kiln_boot").callAttr("init", helper).toString();
                    bridge = py.getModule("kiln_bridge");
                    append(note.isEmpty() ? "Ready." : "C helper problem: " + note);
                } catch (Throwable t) {
                    append("Startup failed: " + t);
                }
            }
        }).start();
    }

    private String joined(List<String> args) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < args.size(); i++) {
            if (i > 0) sb.append(SEP);
            sb.append(args.get(i));
        }
        return sb.toString();
    }

    private void act(String cmd) {
        if (busy) return;
        if (!hasStorage()) { askStorage(); return; }
        if (bridge == null) { toast("Still starting, try again in a moment"); return; }

        String s = src.getText().toString().trim();
        String o = dst.getText().toString().trim();
        String pw = pass.getText().toString();
        if (s.isEmpty()) { toast("Choose a source"); return; }

        List<String> a = new ArrayList<>();
        a.add(cmd);
        if (cmd.equals("pack")) {
            if (o.isEmpty()) { toast("Enter an output name"); return; }
            if (encrypt.isChecked()) {
                if (!pw.equals(pass2.getText().toString())) { toast("Passwords don't match"); return; }
                a.add("-e");
                if (store.isChecked()) a.add("-s");
            } else {
                pw = "";
            }
            if (each.isChecked()) a.add("-i");
            if (force.isChecked()) a.add("-f");
            a.add(o);
            a.add(s);
        } else if (cmd.equals("unpack")) {
            a.add("-v");
            if (force.isChecked()) a.add("-f");
            if (inPlace.isChecked()) {
                a.add("-i");
                a.add(s);
            } else {
                if (o.isEmpty()) { toast("Choose an output folder"); return; }
                a.add(s);
                a.add("-o");
                a.add(o);
            }
        } else {
            a.add(s);
        }

        final String argLine = joined(a);
        final String password = pw;
        setBusy(true);
        append("\n$ kiln " + argLine.replace(SEP, " "));
        new Thread(new Runnable() {
            @Override public void run() {
                int code;
                try {
                    code = bridge.callAttr("run", argLine, password, new LineSink(MainActivity.this)).toInt();
                } catch (Throwable t) {
                    code = 1;
                    append("error: " + t);
                }
                final int rc = code;
                ui.post(new Runnable() {
                    @Override public void run() {
                        setBusy(false);
                        append(rc == 0 ? "Done." : "Failed (exit code " + rc + ").");
                        pass.setText("");
                        pass2.setText("");
                    }
                });
            }
        }).start();
    }

    // ------------------------------------------------------------------ storage + file browser

    private boolean hasStorage() {
        return Build.VERSION.SDK_INT < 30 || Environment.isExternalStorageManager();
    }

    private void askStorage() {
        toast("Allow \"All files access\" for Kiln, then come back");
        Intent i = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:" + getPackageName()));
        startActivity(i);
    }

    private void browse(EditText target, boolean foldersOnly) {
        if (!hasStorage()) { askStorage(); return; }
        String typed = target.getText().toString().trim();
        File f = new File(typed.isEmpty() ? Environment.getExternalStorageDirectory().getPath() : typed);
        while (f != null && !f.isDirectory()) {
            f = f.getParentFile();
        }
        showDir(target, foldersOnly, f != null ? f : Environment.getExternalStorageDirectory());
    }

    private void showDir(final EditText target, final boolean foldersOnly, final File dir) {
        File[] kids = dir.listFiles();
        final List<File> items = new ArrayList<>();
        if (kids != null) {
            for (File k : kids) {
                if (!foldersOnly || k.isDirectory()) items.add(k);
            }
        }
        Collections.sort(items, new Comparator<File>() {
            @Override public int compare(File x, File y) {
                if (x.isDirectory() != y.isDirectory()) return x.isDirectory() ? -1 : 1;
                return x.getName().compareToIgnoreCase(y.getName());
            }
        });
        final File parent = dir.getParentFile();
        final List<String> names = new ArrayList<>();
        if (parent != null) names.add(".. (up)");
        for (File k : items) {
            names.add(k.isDirectory() ? k.getName() + "/" : k.getName());
        }
        final int offset = parent != null ? 1 : 0;

        ListView list = new ListView(this);
        list.setAdapter(new ArrayAdapter<String>(this, android.R.layout.simple_list_item_1, names));
        final AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle(dir.getPath())
                .setView(list)
                .setNegativeButton("Cancel", null)
                .setPositiveButton("Use this folder", new android.content.DialogInterface.OnClickListener() {
                    @Override public void onClick(android.content.DialogInterface d, int which) {
                        target.setText(dir.getPath());
                    }
                })
                .create();
        list.setOnItemClickListener(new AdapterView.OnItemClickListener() {
            @Override public void onItemClick(AdapterView<?> av, View v, int pos, long id) {
                dialog.dismiss();
                if (parent != null && pos == 0) {
                    showDir(target, foldersOnly, parent);
                    return;
                }
                File chosen = items.get(pos - offset);
                if (chosen.isDirectory()) {
                    showDir(target, foldersOnly, chosen);
                } else {
                    target.setText(chosen.getPath());
                }
            }
        });
        dialog.show();
    }
}
