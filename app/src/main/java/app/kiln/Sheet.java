package app.kiln;

import android.animation.Animator;
import android.animation.AnimatorListenerAdapter;
import android.app.Activity;
import android.app.Dialog;
import android.graphics.Color;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsAnimation;
import android.view.WindowManager;
import android.view.animation.DecelerateInterpolator;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/**
 * A modal: a bordered, rounded card in the middle of the screen. The dialog window covers the whole
 * screen and the card follows the keyboard by itself, so nothing jumps when it opens. It fades and
 * scales in, and out again when it closes.
 */
final class Sheet {
    /** Return true to close the sheet, false to keep it open (for example after a bad input). */
    interface Click { boolean run(); }

    private final Activity act;
    private final Dialog dialog;
    private final FrameLayout root;         // fills the whole window; the scrim lives here, unpadded
    private final FrameLayout screen;       // holds the card; padded by the system bars and keyboard
    private final View scrim;
    private final Card card;
    private final LinearLayout body;
    private final Ui.MaxScroll scroll;
    private final List<Button> buttons = new ArrayList<>();
    private Button last;                    // the most recently added button, whatever the final order
    private boolean keyboard, cancelable = true, closing;
    private int imeBottom, navBottom, statusTop;
    private boolean insetsSeen;
    private android.animation.ObjectAnimator glide;
    private Runnable onClose;
    /** Set (by a menu) just before it opens a sheet: that sheet takes it and runs it when the person
     *  backs out of it (Back, a tap outside, or a button with no action such as Cancel / OK). */
    static Runnable nextBack;
    private Runnable back;
    private boolean userCancel;

    /** The card measures itself so it never grows taller than the room above the keyboard. */
    private final class Card extends LinearLayout {
        Card(Activity a) { super(a); }

        @Override
        protected void onMeasure(int w, int h) {
            scroll.limit = -1;
            super.onMeasure(w, h);
            int avail = MeasureSpec.getSize(h);
            int over = getMeasuredHeight() - avail;
            if (avail > 0 && over > 0) {
                scroll.limit = Math.max(Ui.dp(act, 72), scroll.getMeasuredHeight() - over);
                super.onMeasure(w, h);
            }
        }
    }

    Sheet(Activity a, String title) {
        act = a;
        back = nextBack;
        nextBack = null;
        dialog = new Dialog(a, R.style.KilnSheet) {
            @Override
            public void onBackPressed() {
                if (cancelable) {
                    userCancel = true;
                    Sheet.this.dismiss();
                }
            }
        };
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);

        root = new FrameLayout(a);
        screen = new FrameLayout(a);
        scrim = new View(a);
        scrim.setBackgroundColor(Color.BLACK);
        scrim.setAlpha(0f);
        scrim.setOnClickListener(v -> {
            if (cancelable) {
                userCancel = true;
                dismiss();
            }
        });
        screen.setOnClickListener(v -> {                 // a tap beside the card closes it too
            if (cancelable) {
                userCancel = true;
                dismiss();
            }
        });
        // the scrim is a sibling of the padded holder, so the padding can't inset it
        root.addView(scrim, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        root.addView(screen, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        card = new Card(a);
        card.setOrientation(LinearLayout.VERTICAL);
        int pad = Ui.dp(a, 20);
        card.setPadding(pad, pad, pad, pad);
        GradientDrawable g = new GradientDrawable();
        g.setColor(Ui.surface(a));
        g.setCornerRadius(Ui.dp(a, 22));
        g.setStroke(Ui.dp(a, 1), Ui.line(a));
        card.setBackground(g);
        card.setElevation(Ui.dp(a, 8));
        card.setClickable(true);                       // taps on the card don't reach the scrim
        if (title != null && !title.isEmpty()) {
            TextView t = Ui.label(a, title, 19, true, false);
            t.setSingleLine(true);
            t.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            lp.bottomMargin = Ui.dp(a, 12);
            card.addView(t, lp);
        }
        body = new LinearLayout(a);
        body.setOrientation(LinearLayout.VERTICAL);
        scroll = new Ui.MaxScroll(a);
        scroll.addView(body);
        card.addView(scroll, new LinearLayout.LayoutParams(
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
        card.addView(v, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(act, heightDp)));
        return this;
    }

    /** Adds a view below the scrolling part, always in sight above the buttons. */
    Sheet pinned(View v) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.topMargin = Ui.dp(act, 6);
        card.addView(v, lp);
        return this;
    }

    Sheet space(int dp) {
        body.addView(new View(act), new LinearLayout.LayoutParams(1, Ui.dp(act, dp)));
        return this;
    }

    /** A tappable row. Closes the sheet, then runs the action. */
    Sheet option(String label, boolean selected, final Runnable action) {
        return option(0, label, selected, action);
    }

    /** Same, with a small icon in a rounded tile on the left. */
    Sheet option(int icon, String label, boolean selected, final Runnable action) {
        LinearLayout row = new LinearLayout(act);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(Ui.dp(act, 8), Ui.dp(act, 10), Ui.dp(act, 8), Ui.dp(act, 10));
        row.setBackground(Ui.pressable(act, 0, 0, 12));
        if (icon != 0) {
            ImageView iv = new ImageView(act);
            iv.setImageResource(icon);
            Ui.tint(iv, Ui.accent(act));
            iv.setPadding(Ui.dp(act, 8), Ui.dp(act, 8), Ui.dp(act, 8), Ui.dp(act, 8));
            iv.setBackground(Ui.shape(act, act.getColor(R.color.k_chip), 0, 10));
            LinearLayout.LayoutParams ip = new LinearLayout.LayoutParams(Ui.dp(act, 38), Ui.dp(act, 38));
            ip.rightMargin = Ui.dp(act, 14);
            row.addView(iv, ip);
        }
        TextView t = Ui.label(act, label, 16, false, false);
        row.addView(t, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        if (selected) {
            TextView mark = Ui.label(act, "✓", 18, true, false);
            mark.setTextColor(Ui.accent(act));
            row.addView(mark);
        }
        row.setOnClickListener(v -> {
            dismiss();
            if (action != null) action.run();
        });
        LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        rp.bottomMargin = Ui.dp(act, 2);
        body.addView(row, rp);
        return this;
    }

    /** A radio-button row. Picking it closes the sheet, then runs the action. */
    Sheet radio(String label, String detail, boolean selected, final Runnable action) {
        LinearLayout row = new LinearLayout(act);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(Ui.dp(act, 8), Ui.dp(act, 12), Ui.dp(act, 8), Ui.dp(act, 12));
        row.setBackground(Ui.pressable(act, 0, 0, 12));
        View dot = new View(act);
        GradientDrawable ring = new GradientDrawable();
        ring.setShape(GradientDrawable.OVAL);
        ring.setColor(Color.TRANSPARENT);
        ring.setStroke(Ui.dp(act, 2), selected ? Ui.accent(act) : Ui.line(act));
        GradientDrawable core = new GradientDrawable();
        core.setShape(GradientDrawable.OVAL);
        core.setColor(selected ? Ui.accent(act) : Color.TRANSPARENT);
        int in = Ui.dp(act, 5);
        android.graphics.drawable.LayerDrawable ld =
                new android.graphics.drawable.LayerDrawable(new android.graphics.drawable.Drawable[]{ring, core});
        ld.setLayerInset(1, in, in, in, in);
        dot.setBackground(ld);
        LinearLayout.LayoutParams dp = new LinearLayout.LayoutParams(Ui.dp(act, 22), Ui.dp(act, 22));
        dp.rightMargin = Ui.dp(act, 14);
        row.addView(dot, dp);
        LinearLayout col = new LinearLayout(act);
        col.setOrientation(LinearLayout.VERTICAL);
        col.addView(Ui.label(act, label, 16, false, false));
        if (detail != null && !detail.isEmpty()) col.addView(Ui.label(act, detail, 12, false, true));
        row.addView(col, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        row.setOnClickListener(v -> {
            dismiss();
            if (action != null) action.run();
        });
        LinearLayout.LayoutParams rp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        rp.bottomMargin = Ui.dp(act, 2);
        body.addView(row, rp);
        return this;
    }

    /** A thin divider line between groups of options. */
    Sheet divider() {
        View v = new View(act);
        v.setBackgroundColor(Ui.line(act));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, Ui.dp(act, 1));
        lp.topMargin = Ui.dp(act, 6);
        lp.bottomMargin = Ui.dp(act, 6);
        body.addView(v, lp);
        return this;
    }

    // ---- buttons

    Sheet button(String label, int kind, final Click click) {
        Button b = Ui.button(act, label, kind, v -> {
            if (click == null) {
                userCancel = true;
                dismiss();
            } else if (click.run()) {
                dismiss();
            }
        });
        buttons.add(b);
        last = b;
        return this;
    }

    Button lastButton() { return last; }

    Sheet keyboard() { keyboard = true; return this; }

    Sheet cancelable(boolean c) { cancelable = c; return this; }

    Sheet onClose(Runnable r) { onClose = r; return this; }

    // ---- showing

    private int bottomPad() { return Math.max(imeBottom, navBottom) + Ui.dp(act, 12); }

    private void applyInsets() {
        int side = Ui.dp(act, 20);
        screen.setPadding(side, statusTop + Ui.dp(act, 12), side, bottomPad());
    }

    /**
     * The card always sits centred in the room above the keyboard. When the keyboard comes or goes
     * the layout moves at once, and the picture glides there in a single smooth movement, the way a
     * stock dialog does, instead of snapping or trailing the keyboard.
     */
    private void glideBy(float layoutShift) {
        float from = screen.getTranslationY() + layoutShift;
        if (glide != null) glide.cancel();
        screen.setTranslationY(from);
        glide = android.animation.ObjectAnimator.ofFloat(screen, View.TRANSLATION_Y, from, 0f);
        glide.setDuration(Defaults.anim(act, 260));
        glide.setInterpolator(new DecelerateInterpolator(1.8f));
        glide.start();
    }

    Sheet show() {
        if (!buttons.isEmpty()) {
            int screenW0 = act.getResources().getDisplayMetrics().widthPixels;
            int cardW = Math.min(screenW0 - Ui.dp(act, 40), Ui.dp(act, 460));
            boolean stack = buttons.size() > 2;
            if (buttons.size() == 2) {
                // two buttons share a row; if either label wouldn't fit on one line, stack them instead
                int each = (cardW - Ui.dp(act, 40) - Ui.dp(act, 10)) / 2 - Ui.dp(act, 32);
                for (Button b : buttons) {
                    if (b.getPaint().measureText(b.getText().toString()) > each) stack = true;
                }
                // stacked: the main action (added last) goes on top
                if (stack) java.util.Collections.reverse(buttons);
            }
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
            card.addView(row, rp);
        }
        int screenW = act.getResources().getDisplayMetrics().widthPixels;
        int width = Math.min(screenW - Ui.dp(act, 40), Ui.dp(act, 460));
        FrameLayout.LayoutParams cp = new FrameLayout.LayoutParams(width,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER);
        screen.addView(card, cp);

        dialog.setContentView(root);
        Window w = dialog.getWindow();
        w.setBackgroundDrawable(new ColorDrawable(0));
        w.setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        w.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);   // our own scrim does the dimming
        w.setDimAmount(0f);
        w.setWindowAnimations(0);
        w.setDecorFitsSystemWindows(false);               // we place the card ourselves
        w.setStatusBarColor(Color.TRANSPARENT);
        w.setNavigationBarColor(Color.TRANSPARENT);
        w.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                | (keyboard ? WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE
                : WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN));

        root.setOnApplyWindowInsetsListener((v, insets) -> {
            android.graphics.Insets ime = insets.getInsets(WindowInsets.Type.ime());
            android.graphics.Insets bars = insets.getInsets(WindowInsets.Type.systemBars());
            int before = bottomPad();
            imeBottom = ime.bottom;
            navBottom = bars.bottom;
            statusTop = bars.top;
            int after = bottomPad();
            applyInsets();
            if (insetsSeen && after != before && !closing) glideBy((after - before) / 2f);
            insetsSeen = true;
            return WindowInsets.CONSUMED;
        });

        dialog.setOnDismissListener(d -> {
            if (onClose != null) onClose.run();
            if (userCancel && back != null) back.run();
        });
        if (act.isFinishing()) return this;
        dialog.show();

        card.setAlpha(0f);
        card.setScaleX(0.92f);
        card.setScaleY(0.92f);
        card.animate().alpha(1f).scaleX(1f).scaleY(1f)
                .setDuration(Defaults.anim(act, 200)).setInterpolator(new DecelerateInterpolator(1.6f)).start();
        scrim.animate().alpha(0.5f).setDuration(Defaults.anim(act, 200)).start();
        return this;
    }

    void dismiss() {
        if (closing) return;
        closing = true;
        if (!dialog.isShowing() || act.isFinishing()) {
            hardDismiss();
            return;
        }
        scrim.animate().alpha(0f).setDuration(Defaults.anim(act, 140)).start();
        card.animate().alpha(0f).scaleX(0.95f).scaleY(0.95f)
                .setDuration(Defaults.anim(act, 140)).setListener(new AnimatorListenerAdapter() {
                    @Override
                    public void onAnimationEnd(Animator a) { hardDismiss(); }
                }).start();
    }

    private void hardDismiss() {
        try {
            dialog.dismiss();
        } catch (Throwable ignored) {
        }
    }

    boolean isShowing() { return dialog.isShowing() && !closing; }
}
