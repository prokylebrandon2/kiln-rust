package app.kiln;

import android.app.Activity;
import android.content.SharedPreferences;
import android.graphics.Typeface;
import android.os.Bundle;
import android.text.Editable;
import android.text.TextWatcher;
import android.text.method.ArrowKeyMovementMethod;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.HorizontalScrollView;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;
import java.util.Locale;

/** Kiln's own text viewer and editor. Extras: path, edit (boolean), title (optional). */
public class TextActivity extends Activity {
    static final long MAX_BYTES = 1_000_000L;
    private static final int[] SIZES = {11, 13, 15, 17, 20};

    private File file;
    private boolean editable;
    private boolean dirty;
    private boolean wrap = true;
    private int sizeIdx = 2;
    private String saved = "";
    private EditText body;
    private ScrollView vScroll;
    private HorizontalScrollView hScroll;
    private LinearLayout frame, findBar;
    private EditText findField;
    private TextView title, sub;
    private View saveBtn;
    private SharedPreferences prefs;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = getSharedPreferences("kiln", MODE_PRIVATE);
        wrap = prefs.getBoolean("viewer_wrap", true);
        sizeIdx = Math.max(0, Math.min(SIZES.length - 1, prefs.getInt("viewer_size", 2)));
        String path = getIntent().getStringExtra("path");
        editable = getIntent().getBooleanExtra("edit", false);
        if (path == null) {
            finish();
            return;
        }
        file = new File(path);
        build();
        String err = load(state == null);
        if (err != null) {
            Toast.makeText(this, err, Toast.LENGTH_LONG).show();
            finish();
        }
    }

    // ---------------------------------------------------------------- layout

    private void build() {
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setBackgroundColor(Ui.bg(this));

        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(Ui.dp(this, 4), Ui.dp(this, 6), Ui.dp(this, 8), Ui.dp(this, 6));
        bar.addView(Ui.iconButton(this, R.drawable.ic_up, v -> onBackPressed()),
                new LinearLayout.LayoutParams(Ui.dp(this, 48), Ui.dp(this, 48)));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        title = Ui.label(this, file.getName(), 18, true, false);
        title.setSingleLine(true);
        title.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        sub = Ui.label(this, "", 12, false, true);
        sub.setSingleLine(true);
        titles.addView(title);
        titles.addView(sub);
        bar.addView(titles, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        bar.addView(Ui.iconButton(this, R.drawable.ic_search, v -> toggleFind()),
                new LinearLayout.LayoutParams(Ui.dp(this, 48), Ui.dp(this, 48)));
        bar.addView(Ui.iconButton(this, R.drawable.ic_more, v -> optionsSheet()),
                new LinearLayout.LayoutParams(Ui.dp(this, 48), Ui.dp(this, 48)));
        if (editable) {
            android.widget.Button save = Ui.button(this, "Save", Ui.FILLED, v -> save(null));
            saveBtn = save;
            LinearLayout.LayoutParams sp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, Ui.dp(this, 40));
            sp.leftMargin = Ui.dp(this, 4);
            bar.addView(save, sp);
            saveBtn.setEnabled(false);
        }
        page.addView(bar);
        addLine(page);

        findBar = new LinearLayout(this);
        findBar.setOrientation(LinearLayout.HORIZONTAL);
        findBar.setGravity(Gravity.CENTER_VERTICAL);
        findBar.setPadding(Ui.dp(this, 12), Ui.dp(this, 8), Ui.dp(this, 12), Ui.dp(this, 8));
        findField = Ui.field(this, "Find", false);
        findField.setImeOptions(android.view.inputmethod.EditorInfo.IME_ACTION_SEARCH);
        findField.setOnEditorActionListener((v, a, e) -> {
            find(true);
            return true;
        });
        findBar.addView(findField, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        android.widget.Button prev = Ui.button(this, "Prev", Ui.TEXT, v -> find(false));
        android.widget.Button next = Ui.button(this, "Next", Ui.OUTLINE, v -> find(true));
        findBar.addView(prev);
        LinearLayout.LayoutParams np = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        np.leftMargin = Ui.dp(this, 6);
        findBar.addView(next, np);
        findBar.setVisibility(View.GONE);
        page.addView(findBar);

        body = new EditText(this);
        body.setBackground(null);
        body.setTypeface(Typeface.MONOSPACE);
        body.setTextColor(Ui.text(this));
        body.setGravity(Gravity.TOP | Gravity.START);
        body.setPadding(Ui.dp(this, 14), Ui.dp(this, 12), Ui.dp(this, 14), Ui.dp(this, 80));
        body.setInputType(android.text.InputType.TYPE_CLASS_TEXT
                | android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE
                | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        body.setSingleLine(false);
        body.setHorizontallyScrolling(false);
        body.setLinksClickable(false);
        if (!editable) {                      // read-only, but still selectable and searchable
            body.setKeyListener(null);
            body.setTextIsSelectable(true);
            body.setShowSoftInputOnFocus(false);
        }
        body.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void onTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void afterTextChanged(Editable s) { markDirty(); }
        });
        vScroll = new ScrollView(this);
        vScroll.setFillViewport(true);
        hScroll = new HorizontalScrollView(this);
        hScroll.setFillViewport(true);
        frame = new LinearLayout(this);
        page.addView(vScroll, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        setContentView(page);
        applyLayout();
        applySize();
    }

    private void addLine(LinearLayout page) {
        View line = new View(this);
        line.setBackgroundColor(Ui.line(this));
        page.addView(line, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(this, 1)));
    }

    /** Wrapped text lives straight in the vertical scroller; unwrapped text also scrolls sideways. */
    private void applyLayout() {
        ViewGroup old = (ViewGroup) body.getParent();
        if (old != null) old.removeView(body);
        vScroll.removeAllViews();
        hScroll.removeAllViews();
        body.setHorizontallyScrolling(!wrap);
        if (wrap) {
            vScroll.addView(body, new ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        } else {
            hScroll.addView(body, new ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
            vScroll.addView(hScroll, new ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        }
        body.setMinWidth(wrap ? 0 : getResources().getDisplayMetrics().widthPixels);
    }

    private void applySize() { body.setTextSize(SIZES[sizeIdx]); }

    // ---------------------------------------------------------------- loading and saving

    /** @return an error message, or null when the file was loaded. */
    private String load(boolean fresh) {
        long len = file.length();
        if (len > MAX_BYTES) return "This file is larger than 1 MB. Use \"Open with\" instead.";
        byte[] data = new byte[(int) len];
        try (FileInputStream in = new FileInputStream(file)) {
            int n = 0;
            while (n < data.length) {
                int got = in.read(data, n, data.length - n);
                if (got < 0) break;
                n += got;
            }
        } catch (IOException e) {
            return "Couldn't read the file: " + e.getMessage();
        }
        for (byte b : data) {
            if (b == 0) return "This looks like a binary file, not text.";
        }
        String text;
        try {
            text = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(data)).toString();
        } catch (CharacterCodingException e) {
            if (editable) return "This file isn't UTF-8, so editing it here could damage it. "
                    + "Use \"Edit with\" instead.";
            text = new String(data, StandardCharsets.UTF_8);      // view only: show what we can
        }
        saved = text;
        if (fresh) body.setText(text);
        dirty = !body.getText().toString().equals(saved);
        refreshSub();
        return null;
    }

    private void markDirty() {
        if (!editable) return;
        boolean now = !body.getText().toString().equals(saved);
        if (now != dirty) {
            dirty = now;
            refreshSub();
        }
    }

    private void refreshSub() {
        String s = Fmt.size(file.length()) + (editable ? (dirty ? " \u00b7 unsaved changes" : "") : " \u00b7 read-only");
        sub.setText(s);
        if (saveBtn != null) saveBtn.setEnabled(dirty);
    }

    /** Writes next to the file, then swaps it in, so a crash can't leave a half-written file. */
    private void save(final Runnable after) {
        String text = body.getText().toString();
        File tmp = new File(file.getParentFile(), file.getName() + ".kilntmp");
        try (FileOutputStream out = new FileOutputStream(tmp)) {
            out.write(text.getBytes(StandardCharsets.UTF_8));
            out.getFD().sync();
        } catch (IOException e) {
            tmp.delete();
            new Sheet(this, "Couldn't save").text(String.valueOf(e.getMessage()))
                    .button("OK", Ui.FILLED, null).show();
            return;
        }
        try {
            Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING);
        } catch (IOException e) {
            tmp.delete();
            new Sheet(this, "Couldn't save").text(String.valueOf(e.getMessage()))
                    .button("OK", Ui.FILLED, null).show();
            return;
        }
        saved = text;
        dirty = false;
        refreshSub();
        Toast.makeText(this, "Saved", Toast.LENGTH_SHORT).show();
        if (after != null) after.run();
    }

    @Override
    public void onBackPressed() {
        if (findBar.getVisibility() == View.VISIBLE) {
            toggleFind();
            return;
        }
        if (!dirty) {
            finish();
            return;
        }
        new Sheet(this, "Unsaved changes")
                .text("Save your changes to \"" + file.getName() + "\" before leaving?")
                .button("Keep editing", Ui.TEXT, null)
                .button("Discard", Ui.DANGER, () -> {
                    dirty = false;
                    finish();
                    return true;
                })
                .button("Save", Ui.FILLED, () -> {
                    save(this::finish);
                    return true;
                }).show();
    }

    // ---------------------------------------------------------------- find and options

    private void toggleFind() {
        boolean show = findBar.getVisibility() != View.VISIBLE;
        findBar.setVisibility(show ? View.VISIBLE : View.GONE);
        InputMethodManager imm = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
        if (show) {
            findField.requestFocus();
            imm.showSoftInput(findField, 0);
        } else {
            imm.hideSoftInputFromWindow(findField.getWindowToken(), 0);
            body.requestFocus();
        }
    }

    private void find(boolean forward) {
        String q = findField.getText().toString();
        if (q.isEmpty()) return;
        String hay = body.getText().toString().toLowerCase(Locale.ROOT);
        String needle = q.toLowerCase(Locale.ROOT);
        int from = Math.max(body.getSelectionEnd(), 0);
        int at;
        if (forward) {
            at = hay.indexOf(needle, from);
            if (at < 0) at = hay.indexOf(needle);                     // wrap around
        } else {
            int start = Math.max(body.getSelectionStart() - 1, 0);
            at = hay.lastIndexOf(needle, start);
            if (at < 0) at = hay.lastIndexOf(needle);
        }
        if (at < 0) {
            Toast.makeText(this, "Not found", Toast.LENGTH_SHORT).show();
            return;
        }
        body.requestFocus();
        body.setSelection(at, at + needle.length());
        if (body.getLayout() != null) {
            int line = body.getLayout().getLineForOffset(at);
            int y = body.getLayout().getLineTop(line);
            vScroll.scrollTo(0, Math.max(0, y - vScroll.getHeight() / 3));
        }
        InputMethodManager imm = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
        imm.hideSoftInputFromWindow(findField.getWindowToken(), 0);
    }

    private void optionsSheet() {
        final Sheet s = new Sheet(this, "View options");
        s.option(wrap ? "Wrap lines: on" : "Wrap lines: off", wrap, () -> {
            wrap = !wrap;
            prefs.edit().putBoolean("viewer_wrap", wrap).apply();
            applyLayout();
        });
        s.option("Text size: smaller", false, () -> {
            sizeIdx = Math.max(0, sizeIdx - 1);
            prefs.edit().putInt("viewer_size", sizeIdx).apply();
            applySize();
        });
        s.option("Text size: larger", false, () -> {
            sizeIdx = Math.min(SIZES.length - 1, sizeIdx + 1);
            prefs.edit().putInt("viewer_size", sizeIdx).apply();
            applySize();
        });
        s.button("Close", Ui.TEXT, null).show();
    }
}
