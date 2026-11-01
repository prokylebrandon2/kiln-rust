package app.kiln;

import android.app.Activity;
import android.app.Dialog;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/** A modal: a rounded panel centered on the screen (it moves up by itself when the keyboard opens). */
final class Sheet {
    /** Return true to close the sheet, false to keep it open (for example after a bad input). */
    interface Click { boolean run(); }

    private final Activity act;
    private final Dialog dialog;
    private final LinearLayout root, body;
    private final List<Button> buttons = new ArrayList<>();
    private boolean keyboard;
    private Runnable onClose;

    Sheet(Activity a, String title) {
        act = a;
        dialog = new Dialog(a, R.style.KilnSheet);
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
        root = new LinearLayout(a);
        root.setOrientation(LinearLayout.VERTICAL);
        int pad = Ui.dp(a, 20);
        root.setPadding(pad, pad, pad, pad);
        float r = Ui.dp(a, 20);
        GradientDrawable g = new GradientDrawable();
        g.setColor(Ui.surface(a));
        g.setCornerRadius(r);
        root.setBackground(g);
        if (title != null && !title.isEmpty()) {
            TextView t = Ui.label(a, title, 19, true, false);
            t.setSingleLine(true);
            t.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            lp.bottomMargin = Ui.dp(a, 12);
            root.addView(t, lp);
        }
        body = new LinearLayout(a);
        body.setOrientation(LinearLayout.VERTICAL);
        Ui.MaxScroll sv = new Ui.MaxScroll(a);
        sv.addView(body);
        root.addView(sv, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
    }

    // ---- content

    Sheet text(String s) {
        TextView t = Ui.label(act, s, 15, false, true);
        t.setLineSpacing(0, 1.15f);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.bottomMargin = Ui.dp(act, 8);
        body.addView(t, lp);
        return this;
    }

    Sheet content(View v) {
        body.addView(v, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        return this;
    }

    /** Adds a view outside the scroll area (for lists that scroll by themselves). */
    Sheet fixed(View v, int heightDp) {
        root.addView(v, root.getChildCount() - 0, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(act, heightDp)));
        return this;
    }

    Sheet space(int dp) {
        body.addView(new View(act), new LinearLayout.LayoutParams(1, Ui.dp(act, dp)));
        return this;
    }

    /** A tappable row. Closes the sheet, then runs the action. */
    Sheet option(String label, boolean selected, final Runnable action) {
        LinearLayout row = new LinearLayout(act);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(Ui.dp(act, 4), Ui.dp(act, 14), Ui.dp(act, 4), Ui.dp(act, 14));
        row.setBackground(Ui.pressable(act, 0, 0, 8));
        TextView t = Ui.label(act, label, 16, false, false);
        row.addView(t, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        if (selected) {
            TextView mark = Ui.label(act, "\u2713", 18, true, false);
            mark.setTextColor(Ui.accent(act));
            row.addView(mark);
        }
        row.setOnClickListener(v -> {
            dismiss();
            if (action != null) action.run();
        });
        body.addView(row, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        return this;
    }

    // ---- buttons

    Sheet button(String label, int kind, final Click click) {
        Button b = Ui.button(act, label, kind, v -> {
            if (click == null || click.run()) dismiss();
        });
        buttons.add(b);
        return this;
    }

    Button lastButton() { return buttons.isEmpty() ? null : buttons.get(buttons.size() - 1); }

    Sheet keyboard() { keyboard = true; return this; }

    Sheet cancelable(boolean c) { dialog.setCancelable(c); dialog.setCanceledOnTouchOutside(c); return this; }

    Sheet onClose(Runnable r) { onClose = r; return this; }

    // ---- showing

    Sheet show() {
        if (!buttons.isEmpty()) {
            boolean stack = buttons.size() > 2;
            LinearLayout row = new LinearLayout(act);
            row.setOrientation(stack ? LinearLayout.VERTICAL : LinearLayout.HORIZONTAL);
            for (int i = 0; i < buttons.size(); i++) {
                LinearLayout.LayoutParams lp = stack
                        ? new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT)
                        : new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
                if (stack) {
                    lp.topMargin = Ui.dp(act, 8);
                } else if (i > 0) {
                    lp.leftMargin = Ui.dp(act, 10);
                }
                row.addView(buttons.get(i), lp);
            }
            LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            rp.topMargin = Ui.dp(act, 14);
            root.addView(row, rp);
        }
        dialog.setContentView(root);
        Window w = dialog.getWindow();
        w.setBackgroundDrawable(new ColorDrawable(0));
        int screenW = act.getResources().getDisplayMetrics().widthPixels;
        int width = Math.min(screenW - Ui.dp(act, 40), Ui.dp(act, 460));
        w.setLayout(width, ViewGroup.LayoutParams.WRAP_CONTENT);
        w.setGravity(Gravity.CENTER);
        w.setWindowAnimations(0);
        w.addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        w.setDimAmount(0.45f);
        w.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                | (keyboard ? WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE
                : WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN));
        dialog.setOnDismissListener(d -> {
            if (onClose != null) onClose.run();
        });
        if (!act.isFinishing()) dialog.show();
        return this;
    }

    void dismiss() {
        try {
            dialog.dismiss();
        } catch (Throwable ignored) {
        }
    }

    boolean isShowing() { return dialog.isShowing(); }
}
