package app.kiln;

import android.animation.ValueAnimator;
import android.app.Activity;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.drawable.GradientDrawable;
import android.media.ExifInterface;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.GestureDetector;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.animation.DecelerateInterpolator;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;

/**
 * Shows one picture over the whole screen. Pinch to zoom, drag to pan, double-tap to zoom in and
 * back out, tap once to hide or show the bar.
 */
public class ImageActivity extends Activity {
    static final String EXTRA_PATH = "path", EXTRA_CLEANUP = "cleanup";

    private final Handler ui = new Handler(Looper.getMainLooper());
    private ZoomView zoom;
    private LinearLayout topBar;
    private TextView pct;
    private File file;
    private boolean cleanup, chrome = true;
    private int topInset;
    private final Runnable hidePct = () -> pct.animate().alpha(0f).setDuration(Defaults.anim(this, 200)).start();

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        String path = getIntent().getStringExtra(EXTRA_PATH);
        cleanup = getIntent().getBooleanExtra(EXTRA_CLEANUP, false);
        if (path == null) {
            finish();
            return;
        }
        file = new File(path);

        Window w = getWindow();
        w.setDecorFitsSystemWindows(false);
        w.setStatusBarColor(Color.TRANSPARENT);
        w.setNavigationBarColor(Color.TRANSPARENT);
        w.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(Color.BLACK));

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(Color.BLACK);

        zoom = new ZoomView(this);
        zoom.onTap = this::toggleChrome;
        zoom.onScale = this::showPct;
        root.addView(zoom, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        topBar = new LinearLayout(this);
        topBar.setOrientation(LinearLayout.HORIZONTAL);
        topBar.setGravity(Gravity.CENTER_VERTICAL);
        GradientDrawable fade = new GradientDrawable(GradientDrawable.Orientation.TOP_BOTTOM,
                new int[]{0xCC000000, 0x00000000});
        topBar.setBackground(fade);
        ImageButton close = new ImageButton(this);
        close.setImageResource(R.drawable.ic_close);
        close.setImageTintList(android.content.res.ColorStateList.valueOf(Color.WHITE));
        close.setBackground(Ui.pressable(this, 0, 0, 24));
        close.setOnClickListener(v -> finish());
        topBar.addView(close, new LinearLayout.LayoutParams(Ui.dp(this, 48), Ui.dp(this, 48)));
        TextView name = new TextView(this);
        name.setText(file.getName());
        name.setTextColor(Color.WHITE);
        name.setTextSize(16);
        name.setSingleLine(true);
        name.setEllipsize(android.text.TextUtils.TruncateAt.MIDDLE);
        LinearLayout.LayoutParams np = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        np.leftMargin = Ui.dp(this, 6);
        np.rightMargin = Ui.dp(this, 16);
        topBar.addView(name, np);
        root.addView(topBar, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP));

        pct = new TextView(this);
        pct.setTextColor(Color.WHITE);
        pct.setTextSize(13);
        pct.setPadding(Ui.dp(this, 12), Ui.dp(this, 6), Ui.dp(this, 12), Ui.dp(this, 6));
        GradientDrawable pill = new GradientDrawable();
        pill.setColor(0x99000000);
        pill.setCornerRadius(Ui.dp(this, 14));
        pct.setBackground(pill);
        pct.setAlpha(0f);
        FrameLayout.LayoutParams pp = new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM | Gravity.CENTER_HORIZONTAL);
        pp.bottomMargin = Ui.dp(this, 48);
        root.addView(pct, pp);

        root.setOnApplyWindowInsetsListener((v, ins) -> {
            topInset = ins.getInsets(WindowInsets.Type.statusBars()).top;
            topBar.setPadding(Ui.dp(this, 4), topInset, Ui.dp(this, 4), Ui.dp(this, 12));
            return ins;
        });
        setContentView(root);
        WindowInsetsController c = getWindow().getInsetsController();   // needs the content set first
        if (c != null) {
            c.setSystemBarsAppearance(0, WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
            c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        }

        load();
    }

    /** Decodes off the main thread, so the screen opens straight away. */
    private void load() {
        final File f = file;
        new Thread(() -> {
            final Bitmap b = decode(f);
            final int sample = lastSample;
            ui.post(() -> {
                if (isDestroyed()) return;
                if (b == null) {
                    Toast.makeText(this, "This image can't be opened", Toast.LENGTH_SHORT).show();
                    finish();
                    return;
                }
                zoom.setBitmap(b, sample);
            });
        }).start();
    }

    private volatile int lastSample = 1;

    private Bitmap decode(File f) {
        BitmapFactory.Options o = new BitmapFactory.Options();
        o.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(f.getPath(), o);
        if (o.outWidth <= 0 || o.outHeight <= 0) return null;
        int sample = 1;
        while (o.outWidth / sample > 4096 || o.outHeight / sample > 4096
                || (long) (o.outWidth / sample) * (o.outHeight / sample) > 16_000_000L) {
            sample *= 2;
        }
        Bitmap b = null;
        for (int tries = 0; tries < 3 && b == null; tries++) {
            try {
                BitmapFactory.Options d = new BitmapFactory.Options();
                d.inSampleSize = sample;
                b = BitmapFactory.decodeFile(f.getPath(), d);
            } catch (OutOfMemoryError e) {
                sample *= 2;
            }
        }
        lastSample = sample;
        if (b == null) return null;
        int deg = 0;
        try {
            int ori = new ExifInterface(f.getPath()).getAttributeInt(
                    ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL);
            if (ori == ExifInterface.ORIENTATION_ROTATE_90) deg = 90;
            else if (ori == ExifInterface.ORIENTATION_ROTATE_180) deg = 180;
            else if (ori == ExifInterface.ORIENTATION_ROTATE_270) deg = 270;
        } catch (Throwable ignored) {
        }
        if (deg != 0) {
            try {
                Matrix m = new Matrix();
                m.postRotate(deg);
                Bitmap r = Bitmap.createBitmap(b, 0, 0, b.getWidth(), b.getHeight(), m, true);
                if (r != b) b.recycle();
                b = r;
            } catch (Throwable ignored) {
            }
        }
        return b;
    }

    private void toggleChrome() {
        chrome = !chrome;
        topBar.animate().cancel();
        topBar.setVisibility(View.VISIBLE);
        topBar.animate().alpha(chrome ? 1f : 0f).setDuration(Defaults.anim(this, 180))
                .withEndAction(() -> {
                    if (!chrome) topBar.setVisibility(View.INVISIBLE);
                }).start();
        WindowInsetsController c = getWindow().getInsetsController();
        if (c != null) {
            if (chrome) c.show(WindowInsets.Type.systemBars());
            else c.hide(WindowInsets.Type.systemBars());
        }
    }

    private void showPct(float percent) {
        pct.setText(Math.round(percent) + "%");
        ui.removeCallbacks(hidePct);
        pct.animate().cancel();
        pct.setAlpha(1f);
        ui.postDelayed(hidePct, 700);
    }

    @Override
    public void finish() {
        super.finish();
        Ui.noSlideIfOff(this);
    }

    @Override
    protected void onDestroy() {
        ui.removeCallbacks(hidePct);
        super.onDestroy();
        if (cleanup && isFinishing() && file != null && file.getParentFile() != null) {
            deleteTree(file.getParentFile());               // the temporary copy taken out of an archive
        }
    }

    private static void deleteTree(File f) {
        File[] kids = f.listFiles();
        if (kids != null) for (File k : kids) deleteTree(k);
        f.delete();
    }

    // ------------------------------------------------------------------ the zoomable picture

    private static final class ZoomView extends View {
        Runnable onTap;
        interface PctListener { void pct(float p); }
        PctListener onScale;

        private final Matrix m = new Matrix();
        private final float[] v = new float[9];
        private final RectF rect = new RectF();
        private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG | Paint.ANTI_ALIAS_FLAG);
        private final ScaleGestureDetector scaler;
        private final GestureDetector taps;
        private Bitmap bmp;
        private int sample = 1;
        private float fit = 1f, maxScale = 1f;
        private ValueAnimator anim;
        private boolean scaling;

        ZoomView(Context c) {
            super(c);
            scaler = new ScaleGestureDetector(c, new ScaleGestureDetector.SimpleOnScaleGestureListener() {
                @Override
                public boolean onScaleBegin(ScaleGestureDetector d) {
                    cancelAnim();
                    scaling = true;
                    return true;
                }

                @Override
                public boolean onScale(ScaleGestureDetector d) {
                    float cur = scale();
                    float target = Math.max(fit * 0.9f, Math.min(maxScale, cur * d.getScaleFactor()));
                    float f = target / cur;
                    m.postScale(f, f, d.getFocusX(), d.getFocusY());
                    clamp(false);
                    notifyScale();
                    invalidate();
                    return true;
                }

                @Override
                public void onScaleEnd(ScaleGestureDetector d) {
                    scaling = false;
                    if (scale() < fit) settle();            // let go below "fit": glide back to it
                }
            });
            taps = new GestureDetector(c, new GestureDetector.SimpleOnGestureListener() {
                @Override
                public boolean onDown(MotionEvent e) { return true; }

                @Override
                public boolean onSingleTapConfirmed(MotionEvent e) {
                    if (onTap != null) onTap.run();
                    return true;
                }

                @Override
                public boolean onDoubleTap(MotionEvent e) {
                    if (bmp == null) return true;
                    float cur = scale();
                    float target = cur > fit * 1.3f ? fit : Math.min(maxScale, Math.max(fit * 2.5f, 1f));
                    animateTo(target, e.getX(), e.getY());
                    return true;
                }

                @Override
                public boolean onScroll(MotionEvent a, MotionEvent b, float dx, float dy) {
                    if (bmp == null || scaling) return true;
                    cancelAnim();
                    m.postTranslate(-dx, -dy);
                    clamp(true);
                    invalidate();
                    return true;
                }
            });
        }

        void setBitmap(Bitmap b, int sampleSize) {
            bmp = b;
            sample = Math.max(1, sampleSize);
            resetFit();
            setAlpha(0f);
            animate().alpha(1f).setDuration(Defaults.anim(getContext(), 220)).start();
        }

        @Override
        protected void onSizeChanged(int w, int h, int ow, int oh) {
            super.onSizeChanged(w, h, ow, oh);
            if (bmp != null) resetFit();
        }

        private void resetFit() {
            if (bmp == null || getWidth() == 0 || getHeight() == 0) return;
            fit = Math.min((float) getWidth() / bmp.getWidth(), (float) getHeight() / bmp.getHeight());
            // up to about 8x the fitted size, and never less than showing real pixels
            maxScale = Math.max(fit * 8f, 2f);
            m.reset();
            m.postScale(fit, fit);
            m.postTranslate((getWidth() - bmp.getWidth() * fit) / 2f,
                    (getHeight() - bmp.getHeight() * fit) / 2f);
            invalidate();
        }

        private float scale() {
            m.getValues(v);
            return v[Matrix.MSCALE_X];
        }

        private void notifyScale() {
            if (onScale != null) onScale.pct(scale() * sample * 100f);
        }

        /** Keeps the picture from being dragged away; small pictures stay centred. */
        private void clamp(boolean allowEdgeSlack) {
            if (bmp == null) return;
            rect.set(0, 0, bmp.getWidth(), bmp.getHeight());
            m.mapRect(rect);
            float dx = 0, dy = 0;
            int vw = getWidth(), vh = getHeight();
            if (rect.width() <= vw) dx = (vw - rect.width()) / 2f - rect.left;
            else if (rect.left > 0) dx = -rect.left;
            else if (rect.right < vw) dx = vw - rect.right;
            if (rect.height() <= vh) dy = (vh - rect.height()) / 2f - rect.top;
            else if (rect.top > 0) dy = -rect.top;
            else if (rect.bottom < vh) dy = vh - rect.bottom;
            m.postTranslate(dx, dy);
        }

        private void cancelAnim() {
            if (anim != null) {
                anim.cancel();
                anim = null;
            }
        }

        private void settle() {
            animateTo(fit, getWidth() / 2f, getHeight() / 2f);
        }

        /** Glides to a new zoom level, keeping the point under (px, py) where it is. */
        private void animateTo(final float target, final float px, final float py) {
            cancelAnim();
            final float start = scale();
            anim = ValueAnimator.ofFloat(0f, 1f);
            anim.setDuration(Defaults.anim(getContext(), 260));
            anim.setInterpolator(new DecelerateInterpolator(1.8f));
            anim.addUpdateListener(a -> {
                float t = (float) a.getAnimatedValue();
                float want = start + (target - start) * t;
                float f = want / scale();
                m.postScale(f, f, px, py);
                clamp(false);
                notifyScale();
                invalidate();
            });
            anim.start();
        }

        @Override
        public boolean onTouchEvent(MotionEvent e) {
            scaler.onTouchEvent(e);
            if (!scaler.isInProgress()) taps.onTouchEvent(e);
            return true;
        }

        @Override
        protected void onDraw(Canvas c) {
            if (bmp != null) c.drawBitmap(bmp, m, paint);
        }
    }
}
